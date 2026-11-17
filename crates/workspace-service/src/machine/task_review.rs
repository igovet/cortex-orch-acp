//! Owner acceptance is a private overlay, never a runtime completion or new check receipt.
use super::{context, git::Git, native::*, settings, task_facts};
use cortex_core::{OpenMode, Store};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;

fn unavailable() -> Refused {
    refuse(
        "result_unavailable",
        "Only a completed runtime result can be accepted.",
    )
}
fn validate(
    db: &Connection,
    store: &Store,
    root: &Path,
    project: &str,
    task: &str,
    record: &Value,
    p: &Value,
    native: Option<&str>,
) -> Result<(Value, [u64; 3])> {
    let execution = db.query_row(
        "SELECT e.execution_id FROM workspace_tasks t
        JOIN workspace_executions e ON e.execution_id=t.execution_id AND e.task_id=t.task_id
        JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
        JOIN tasks base ON base.id=r.task_id
        JOIN acp_host_sessions s ON s.session_id=e.acp_session_id
        JOIN task_owners o ON o.task_id=r.task_id AND o.epoch=e.owner_epoch
        WHERE t.project_id=? AND t.task_id=? AND r.state='COMPLETED' AND r.revision>=0
        AND (r.blocker IS NULL OR r.blocker='') AND base.project_root=?
        AND s.project_root=base.project_root AND s.task_id=r.task_id
        AND NOT EXISTS(SELECT 1 FROM workspace_starts WHERE execution_id=e.execution_id AND state='unknown')",
        (project, task, root.to_string_lossy().as_ref()), |r| r.get::<_, String>(0))
        .optional().map_err(|e| context::core(e.into()))?.ok_or_else(unavailable)?;
    let (actions, _, warnings, counts) = task_facts::result_evidence_db(
        db, store, root, project, &execution, native, task_facts::own_writes(record),
    )
    .map_err(|e| {
        if e.code() == "result_unavailable" {
            refuse("machine_tool_failed", "WorkspaceError")
        } else {
            context::core(e)
        }
    })?;
    if actions["digest"] != p["digest"] || warnings {
        return Err(refuse(
            "stale_result",
            "This result or its workspace changed. Rerun checks and review the new result.",
        ));
    }
    if actions["accept"] != true {
        return Err(refuse(
            "checks_required",
            actions["reason"]
                .as_str()
                .unwrap_or("Result evidence is unavailable."),
        ));
    }
    if record["requiresFreshResult"]
        == json!({"resultId":actions["resultId"],"version":actions["version"]})
    {
        return Err(refuse(
            "review_required",
            "The source changed after this result. Run fresh checks and review the new result.",
        ));
    }
    if python_text(&p["version"]) != actions["version"].as_str().unwrap_or("") {
        return Err(refuse(
            "stale_result",
            "This result version changed. Refresh and review the current result.",
        ));
    }
    if actions["acknowledgeOpenChecks"] == true && p["acknowledgeOpenChecks"] != true {
        return Err(refuse(
            "acknowledgement_required",
            "Explicitly acknowledge the recorded technical blockers before accepting.",
        ));
    }
    Ok((actions, counts))
}
pub async fn accept(
    root: &Path,
    project: &str,
    task: &str,
    state: &Path,
    product: &mut Value,
    p: &Value,
    git: &mut Git,
) -> Result<Value> {
    let store = context::open(root, OpenMode::RuntimeValidate)?.ok_or_else(|| {
        refuse(
            "result_unavailable",
            "Activate Cortex and review its runtime result first.",
        )
    })?;
    let record = product["tasks"][task].clone();
    // Native verification is bound in the native digest domain (no effect).
    let native = task_facts::native_observation(root).await;
    let native = native.as_deref();
    // Preserve result/version refusal order before resolving the checkout.
    store
        .read(|db| {
            Ok(validate(
                db, &store, root, project, task, &record, p, native,
            ))
        })
        .map_err(context::core)??;
    checkout_shape(&record)?;
    let path = super::retention::task_path(root, &record, git).await?;
    store.write(|db| Ok((|| {
        // Start/owner replacement cannot race evidence validation and overlay replacement.
        let (_, counts) = validate(db, &store, root, project, task, &record, p, native)?;
        let observed = super::legacy_source::capture(&path).map_err(context::core)?
            .ok_or_else(|| refuse("workspace_unobservable", "The workspace could not be observed; acceptance was not recorded."))?;
        let record = &mut product["tasks"][task];
        record["acceptedRevision"] = json!(observed);
        record["status"] = json!("ready_to_merge");
        record["acceptance"] = json!({"version":p["version"],"digest":p["digest"],"at":now_ms(),
            "open":{"failed":counts[0],"not_run":counts[1],"unverified":counts[2]}});
        // No retained-provider error may be returned after this mutation boundary.
        settings::write_json(&product_path(state, project)?, product)?;
        Ok(json!({"done":true,"reclamation":product["tasks"][task]["reclamation"]}))
    })())).map_err(context::core)?
}
pub async fn answer_archive(
    root: &Path,
    state: &Path,
    record: &mut Value,
    p: &Value,
    git: &mut Git,
) -> Result<()> {
    let at = now_ms();
    if p["discard"] == true {
        checkout_shape(record)?;
        let path = super::retention::task_path(root, record, git).await?;
        let observed = super::legacy_source::capture(&path)
            .map_err(context::core)?
            .ok_or_else(|| {
                refuse(
                    "workspace_unobservable",
                    "The work could not be observed; no discard decision was recorded.",
                )
            })?;
        record["discardRevision"] = json!(observed);
        record["work"] = json!("discard_confirmed");
        record["archive"] = json!({"at":at,"previousStatus":"done"});
        record["archiveQuestion"] = json!(false);
    } else {
        let days = settings::machine(state)?["autoArchiveDays"].clone();
        let until = if let Some(n) = days.as_i64().or_else(|| days.as_bool().map(i64::from)) {
            let n = at as i128 + n as i128 * 86_400_000;
            if let Ok(n) = i64::try_from(n) {
                json!(n)
            } else {
                // Python would persist an unbounded integer; the saved presentation
                // overlay cannot represent it, so no postponement is recorded.
                return Err(refuse(
                    "archive_interval_unsupported",
                    "The configured archive interval is outside the supported range; no postponement was recorded.",
                ));
            }
        } else if days.as_u64().is_some() {
            return Err(refuse(
                "archive_interval_unsupported",
                "The configured archive interval is outside the supported range; no postponement was recorded.",
            ));
        } else if let Some(n) = days.as_f64() {
            serde_json::Number::from_f64(at as f64 + n * 86_400_000.0)
                .map(Value::Number)
                .ok_or_else(|| refuse("machine_tool_failed", "ValueError"))?
        } else {
            return Err(refuse("machine_tool_failed", "TypeError"));
        };
        record["archiveQuestion"] = json!(false);
        record["archiveAskAfter"] = until;
    }
    Ok(())
}
fn checkout_shape(record: &Value) -> Result<()> {
    let checkout = &record["checkout"];
    if super::workers::truthy(checkout)
        && (!checkout.is_object()
            || !checkout["kind"].is_string()
            || checkout["kind"] != "main"
                && (!checkout["path"].is_string() || !checkout["branch"].is_string()))
    {
        // An incomplete checkout record has no observable worktree. Refuse before
        // any effect; nothing is discarded, accepted or recorded.
        return Err(refuse(
            "unknown_checkout",
            "The task worktree is missing or its checkout record is incomplete.",
        ));
    }
    Ok(())
}
fn python_text(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}
/// Live N1: a finished native task showed "Result evidence is unavailable" and
/// Accept was disabled. Its review actions and identity must come from the
/// runtime evidence bound in the native verifier's digest domain (Python
/// `result_projection`), and Accept must take exactly the presented result.
#[cfg(test)]
mod native_acceptance_tests {
    use super::*;
    use crate::{actor, native_runtime::Runtime, native_runtime::UnqualifiedRoutes};
    use cortex_core::{lifecycle, workspace};
    use sha2::{Digest, Sha256};
    use std::sync::Arc;

    async fn completed() -> (std::path::PathBuf, Arc<Runtime>, String) {
        let root =
            std::env::temp_dir().join(format!("native-accept-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("work.txt"), "after").unwrap();
        assert!(std::process::Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        // The digest the native verifier records for this folder.
        let digest = crate::native_effects::observe(root.clone())
            .await
            .unwrap()
            .project_digest
            .unwrap();
        let draft = runtime.host.executor.publication(move |store| {
            let command = json!({"protocol":{"major":1,"minor":8},"requestId":"draft",
                "projectId":"project","actorId":actor(),"operation":"tasks.create","target":"project",
                "expectedVersion":"0","ownerEpoch":null,
                "payload":{"description":"Accept a finished result","checkout":{"kind":"main"}}});
            let draft = workspace::create_draft(store, "project", &actor(), &command)?.receipt;
            let start = json!({"protocol":{"major":1,"minor":8},"requestId":"start",
                "projectId":"project","actorId":actor(),"operation":"tasks.start",
                "target":draft["target"],"expectedVersion":draft["entityVersion"],
                "ownerEpoch":null,"payload":{}});
            let started = workspace::start_native(store, "project", &actor(), &start)?;
            let task = started["task"].as_str().unwrap().to_owned();
            // Exactly what a native completion retains: published worker and
            // approving reviewer reports, and bound runtime verification.
            let folder = store.key().directory().join(&task);
            std::fs::create_dir_all(&folder)?;
            let mut digests = Vec::new();
            for name in ["work.md", "review.md"] {
                let body = format!("# {name}\n\nObserved disposable fixture.\n");
                std::fs::write(folder.join(name), &body)?;
                digests.push(format!("{:x}", Sha256::digest(body.as_bytes())));
            }
            store.write(|db| {
                for (id, name) in [("r_work", "work.md"), ("r_review", "review.md")] {
                    db.execute("INSERT INTO reports VALUES (?,?,'report',?,'now')", (id, &task, name))?;
                }
                db.execute("INSERT INTO runtime_nodes VALUES (?,0,'work','worker-backend-dev','Work','[]','[\"work.txt\"]','execution','completed','a-work',?)",
                    (&task, json!({"report_ref":"r_work","report_digest":digests[0],"summary":"Wrote work.txt.","checks":[{"name":"content check","outcome":"passed","tool_call_ref":"native-command:w"}]}).to_string()))?;
                db.execute("INSERT INTO runtime_nodes VALUES (?,0,'review','worker-code-reviewer','Review','[\"work\"]','[\"work.txt\"]','review','completed','a-review',?)",
                    (&task, json!({"report_ref":"r_review","report_digest":digests[1],"verdict":"APPROVE","checks":[],"review_independence":{"status":"warning","same_engine_as_reviewed":true,"warning":"The reviewer runs on the same model as the work it reviews."}}).to_string()))?;
                for (key, channel) in [("a-work", "ch-work"), ("a-review", "ch-review")] {
                    db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,assignment_id,attempt_id,role_id,step,status,channel_id,owner_epoch,created_at,updated_at) VALUES (?,?,?,?,'role','ready','ready',?,1,'now','now')",
                        (key, &task, key, format!("att-{key}"), channel))?;
                    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at) VALUES (?,'h',?,'p','role',?,?,1,'now')",
                        (channel, &task, key, format!("att-{key}")))?;
                }
                lifecycle::event_tx(db, &task, "NATIVE_COMMAND_FINISHED", json!({"receipt":{"revision":0,"argv":["/bin/true"],"revision_digest":digest,
                    "tool_call_ref":"native-command:v","observation":{"complete":true,"exit_code":0}}}))?;
                lifecycle::event_tx(db, &task, "NATIVE_PROJECT_WRITTEN", json!({"channel_id":"ch-work","path":"work.txt","sha256":"s","add":3,"del":1}))?;
                lifecycle::event_tx(db, &task, "NATIVE_PROJECT_WRITTEN", json!({"channel_id":"ch-work","path":"work.txt","sha256":"t","add":2,"del":0}))?;
                // What those writes' requests recorded: an unfinished attempt, the first
                // write's whole-text diff, and a clipped diff for the second.
                let reply = |sha: &str, diff: &str, truncated: bool| json!({"complete":true,"reply":{"content":[],"isError":false,
                    "structuredContent":{"path":"work.txt","sha256":sha,"diff":diff,"diff_truncated":truncated,"outcome":"written"}}}).to_string();
                for (key, result) in [
                    ("k0", json!({"complete":false,"path":"work.txt","reply":{"structuredContent":{"path":"work.txt","sha256":"s","diff":"--- work.txt\n+++ work.txt\n@@ -1,0 +1,1 @@\n+WRONG\n","diff_truncated":false}}}).to_string()),
                    ("k1", reply("s", "--- work.txt\n+++ work.txt\n@@ -1,2 +1,4 @@\n-a\n-b\n+a\n+B\n+c\n+d\n", false)),
                    ("k2", reply("t", "--- work.txt\n+++ work.txt\n@@ -1,4 +1,6 @@\n-a\n", true)),
                ] {
                    db.execute("INSERT INTO runtime_requests VALUES (?,?,'ch-work','project_edit','d',?)", (&task, key, result))?;
                }
                db.execute("UPDATE runtime_tasks SET state='COMPLETED',synthesis='Fixture result.',plan_json=? WHERE task_id=?",
                    (json!({"verification":[["/bin/true"]]}).to_string(), &task))?;
                lifecycle::event_tx(db, &task, "VERIFICATION_PASSED", json!({"revision":0,"revision_digest":digest,
                    "checks":[{"argv":["/bin/true"],"complete":true,"exit_code":0,"revision_digest":digest}]}))?;
                Ok(())
            })?;
            Ok(draft["target"].as_str().unwrap().to_owned())
        }).unwrap().await.unwrap().unwrap();
        (root, runtime, draft)
    }
    async fn presented(runtime: &Runtime, draft: &str) -> Value {
        let facts = runtime
            .owner_facts(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({}),
            )
            .await
            .unwrap();
        facts["product"]["tasks"][draft].clone()
    }
    #[tokio::test]
    async fn finished_native_result_is_presented_acceptable_and_accept_records_it() {
        let (root, runtime, draft) = completed().await;
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["accept"], true, "{task}");
        assert_eq!(task["result"]["acceptanceAvailable"], true, "{task}");
        assert_eq!(task["result"]["kind"], "work");
        assert_eq!(task["result"]["digest"], task["workspaceDigest"]);
        // The same-model review is a visible note, not unavailable evidence.
        assert_eq!(task["result"]["warnings"][0], "The reviewer runs on the same model as the work it reviews.");
        assert!(task["reviewActions"]["workspaceChanged"].is_null());
        assert_eq!(task["result"]["checks"][0]["source"], "runtime");
        // Private store area: an empty directory elsewhere in the project is a
        // project change and would make the recorded digest stale.
        let state = root.join(".cortex-dev/acceptance-state");
        std::fs::create_dir(&state).unwrap();
        let saved = |draft: &str| {
            let mut product = json!({"tasks":{},"sprints":[]});
            product["tasks"][draft] = json!({"checkout":{"kind":"main"}});
            product
        };
        let mut product = saved(&draft);
        let p = json!({"action":"accept","taskId":draft,"digest":task["result"]["digest"],
            "version":task["result"]["sourceVersion"],"acknowledgeOpenChecks":false});
        let accepted = accept(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &p,
            &mut Git::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("{}: {}", e.code, e.message));
        assert_eq!(accepted["done"], true);
        assert_eq!(product["tasks"][&draft]["status"], "ready_to_merge");
        assert_eq!(
            product["tasks"][&draft]["acceptance"]["digest"],
            p["digest"]
        );
        // Files changed after the result: no longer acceptable, and Accept of the
        // old identity is refused without recording anything.
        std::fs::write(root.join("work.txt"), "owner change").unwrap();
        let drifted = presented(&runtime, &draft).await;
        assert_eq!(drifted["result"]["acceptanceAvailable"], false);
        assert_eq!(drifted["reviewActions"]["workspaceChanged"], true);
        assert_ne!(drifted["result"]["digest"], drifted["workspaceDigest"]);
        assert_eq!(drifted["reviewActions"]["unblockAction"], "request_changes");
        let mut fresh = saved(&draft);
        let refused = accept(
            &root,
            "project",
            &draft,
            &state,
            &mut fresh,
            &p,
            &mut Git::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(refused.code, "stale_result");
        assert!(fresh["tasks"][&draft]["acceptance"].is_null());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn native_workers_report_their_edits_result_and_checks() {
        let (root, runtime, draft) = completed().await;
        let facts = runtime
            .owner_facts("project", json!({"tasks":{},"sprints":[],"messages":[]}), json!({}))
            .await
            .unwrap();
        let workers = facts["workers"].as_array().unwrap();
        let work = workers.iter().find(|w| w["role"] == "backend-dev").unwrap();
        assert_eq!(work["taskId"], draft);
        assert_eq!(work["result"]["summary"], "Wrote work.txt.");
        assert_eq!(work["result"]["reportId"], "r_work");
        assert_eq!(work["result"]["checks"][0]["source"], "worker");
        assert_eq!(work["result"]["edits"], json!([{"path":"work.txt","add":5,"del":1}]));
        let edits: Vec<_> = work["activity"].as_array().unwrap().iter().filter(|a| a["kind"] == "edit").collect();
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0]["code"], "work.txt");
        assert_eq!(edits[0]["diff"]["add"], 3);
        // The recorded write opens on its changed lines; a clipped record shows its counts only.
        assert_eq!(edits[0]["diff"]["lines"], json!(["@@ -1,2 +1,4 @@", " a", "-b", "+B", "+c", "+d"]));
        assert_eq!(edits[1]["diff"]["lines"], json!([]));
        assert_eq!(edits[1]["diff"]["add"], 2);
        // The reviewer made no edits.
        let review = workers.iter().find(|w| w["role"] == "code-reviewer").unwrap();
        assert_eq!(review["result"]["edits"], json!([]));
        assert!(review["activity"].as_array().unwrap().iter().all(|a| a["kind"] != "edit"));
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Owner store 2026-11-17: a worker card read "This worker reported no result" with
    /// no reason after its result submissions were refused. Its recorded refusals (code
    /// only) are on the card; a worker with none keeps its recorded shape.
    #[tokio::test]
    async fn a_workers_refused_results_are_on_its_card() {
        let (root, runtime, _) = completed().await;
        runtime
            .host
            .executor
            .publication(|store| {
                store.write(|db| {
                    let (task, attempt): (String, String) = db.query_row(
                        "SELECT task_id,attempt_id FROM delegation_provisioning WHERE idempotency_key='a-work'",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    for code in ["open_failed_check", "omitted_failed_check"] {
                        lifecycle::event_tx(db, &task, "WORKER_RESULT_REFUSED", json!({"channel_id":"c","attempt":attempt,"assignment_ref":"a","code":code}))?;
                    }
                    Ok(())
                })?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let facts = runtime
            .owner_facts("project", json!({"tasks":{},"sprints":[],"messages":[]}), json!({}))
            .await
            .unwrap();
        let workers = facts["workers"].as_array().unwrap();
        let work = workers.iter().find(|w| w["role"] == "backend-dev").unwrap();
        assert_eq!(work["resultRefusals"], json!({"count":2,"code":"omitted_failed_check"}));
        let review = workers.iter().find(|w| w["role"] == "code-reviewer").unwrap();
        assert!(review.get("resultRefusals").is_none());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Runs one write against the fixture's runtime task and its session.
    async fn mutate(runtime: &Runtime, f: impl FnOnce(&Connection, &str, &str) -> cortex_core::CoreResult<()> + Send + 'static) {
        runtime
            .host
            .executor
            .publication(move |store| {
                store.write(|db| {
                    let (task, session): (String, String) = db.query_row(
                        "SELECT runtime_task_id,acp_session_id FROM workspace_executions LIMIT 1",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    f(db, &task, &session)
                })?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }
    fn page_sequences(worker: &Value) -> Vec<i64> {
        worker["activity"].as_array().unwrap().iter().map(|a| a["sequence"].as_i64().unwrap()).collect()
    }

    /// Owner store 2026-11-17: a long native task (many commands, guidance messages, workers and
    /// check receipts) failed every read with `native_facts_limit`. Nothing is refused for size:
    /// a worker's page is a window with a cursor to its older rows, every row is reachable, and a
    /// result whose receipts exceed the read bound is marked and never offered for Accept.
    #[tokio::test]
    async fn a_large_task_reads_in_pages_and_is_never_refused() {
        let (root, runtime, draft) = completed().await;
        let digest = crate::native_effects::observe(root.clone()).await.unwrap().project_digest.unwrap();
        mutate(&runtime, move |db, task, _| {
            for i in 0..250 {
                let reference = format!("native-command:l{i}");
                lifecycle::event_tx(db, task, "NATIVE_COMMAND_STARTED", json!({"ref":reference,"argv":["/bin/echo",i.to_string()],"channel_id":"ch-work"}))?;
                lifecycle::event_tx(db, task, "NATIVE_COMMAND_FINISHED", json!({"receipt":{"revision":0,"argv":["/bin/echo",i.to_string()],"revision_digest":digest,
                    "tool_call_ref":reference,"observation":{"complete":true,"exit_code":0}}}))?;
            }
            for i in 0..120 {
                lifecycle::event_tx(db, task, "NATIVE_GUIDANCE_QUEUED", json!({"request":format!("g{i}"),"worker":"a-work","text":"keep going"}))?;
            }
            for i in 0..120 {
                let (key, channel) = (format!("a-extra{i}"), format!("ch-extra{i}"));
                db.execute("INSERT INTO runtime_nodes VALUES (?,0,?,'worker-backend-dev','Extra','[]','[]','execution','completed',?,NULL)", (task, format!("extra{i}"), &key))?;
                db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,assignment_id,attempt_id,role_id,step,status,channel_id,owner_epoch,created_at,updated_at) VALUES (?,?,?,?,'role','ready','ready',?,1,'now','now')",
                    (&key, task, &key, format!("att-{key}"), &channel))?;
            }
            Ok(())
        })
        .await;
        let facts = runtime.owner_facts("project", json!({"tasks":{},"sprints":[],"messages":[]}), json!({})).await.unwrap();
        // Every guidance message is present; the roster holds all 122 workers.
        assert_eq!(facts["product"]["messages"].as_array().unwrap().len(), 120);
        assert_eq!(facts["workers"].as_array().unwrap().len(), 122);
        assert!(facts["partial"]["messages"].is_null() && facts["partial"]["workers"].is_null());
        let work = facts["workers"].as_array().unwrap().iter().find(|w| w["id"] == "a-work").unwrap().clone();
        assert_eq!(work["activity"].as_array().unwrap().len(), 200);
        assert_eq!(work["activityPartial"], true);
        // The pages together hold every command and both edits once, newest page first.
        let mut all = page_sequences(&work);
        let mut cursor = work["activityCursor"].as_str().map(str::to_owned);
        let mut pages = 1;
        while let Some(next) = cursor {
            let page = runtime.owner_facts("project", json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({"taskIds":[draft],"workerId":"a-work","cursor":next})).await.unwrap();
            let worker = page["workers"].as_array().unwrap()[0].clone();
            assert!(page_sequences(&worker).iter().all(|s| !all.contains(s)));
            all.extend(page_sequences(&worker));
            cursor = worker["activityCursor"].as_str().map(str::to_owned);
            pages += 1;
            assert!(pages < 10);
        }
        let mut unique = all.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), all.len());
        assert_eq!(all.len(), 250 + 2);
        assert_eq!(pages, 2);
        // The result's receipts are within the bound here, so Accept stays decided by the review.
        assert!(facts["product"]["tasks"][&draft]["result"]["checksPartial"].is_null());
        // More receipts than one read presents: marked, and Accept is unavailable with a reason.
        let digest = crate::native_effects::observe(root.clone()).await.unwrap().project_digest.unwrap();
        mutate(&runtime, move |db, task, _| {
            for i in 0..1005 {
                lifecycle::event_tx(db, task, "NATIVE_COMMAND_FINISHED", json!({"receipt":{"revision":0,"argv":["/bin/echo",format!("c{i}")],"revision_digest":digest,
                    "tool_call_ref":format!("native-command:r{i}"),"observation":{"complete":true,"exit_code":0}}}))?;
            }
            Ok(())
        })
        .await;
        let facts = runtime.owner_facts("project", json!({"tasks":{},"sprints":[],"messages":[]}), json!({})).await.unwrap();
        let task = &facts["product"]["tasks"][&draft];
        assert_eq!(task["result"]["checksPartial"], true);
        assert_eq!(task["result"]["acceptanceAvailable"], false);
        assert_eq!(task["reviewActions"]["accept"], false);
        assert!(task["reviewActions"]["reason"].as_str().unwrap().contains("check receipts"));
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Nothing a worker did is out of reach: tool steps, narration and queued messages past one page are read behind the cursor.
    #[tokio::test]
    async fn older_steps_narration_and_messages_are_reachable_by_cursor() {
        let (root, runtime, draft) = completed().await;
        mutate(&runtime, |db, task, session| {
            for i in 0..450 {
                lifecycle::event_tx(db, task, "NATIVE_GUIDANCE_QUEUED", json!({"request":format!("g{i}"),"worker":"a-work","text":"keep going"}))?;
                let update = json!({"sessionUpdate":"tool_call","toolCallId":format!("att-a-work:c{i}"),"title":"Read a.ts","kind":"read","rawInput":{"path":"a.ts"},"status":"completed"});
                db.execute("INSERT INTO host_updates VALUES (?,?,?,'2026-11-17T10:00:01.000000+00:00')", (session, 10_000 + i as i64, update.to_string()))?;
                db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,message_id,created_at) VALUES (?,?,'agent_message',?,?,'2026-11-17T10:00:00.000000+00:00')",
                    (session, i as i64, format!("note {i}"), format!("att-a-work:n{i}")))?;
            }
            Ok(())
        })
        .await;
        let read = |params: Value| {
            let runtime = runtime.clone();
            async move { runtime.owner_facts("project", json!({"tasks":{},"sprints":[],"messages":[]}), params).await.unwrap() }
        };
        let first = read(json!({})).await;
        let mut messages: Vec<String> = first["product"]["messages"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_owned()).collect();
        let mut worker = first["workers"].as_array().unwrap().iter().find(|w| w["id"] == "a-work").unwrap().clone();
        let (mut tools, mut notes) = (0, 0);
        let mut pages = 0;
        loop {
            for a in worker["activity"].as_array().unwrap() {
                match a["kind"].as_str().unwrap() { "tool" => tools += 1, "note" => notes += 1, _ => {} }
            }
            messages.extend(worker["messages"].as_array().into_iter().flatten().map(|m| m["id"].as_str().unwrap().to_owned()));
            let Some(cursor) = worker["activityCursor"].as_str().map(str::to_owned) else { break };
            pages += 1;
            assert!(pages < 30, "paging must end");
            let page = read(json!({"taskIds":[draft],"workerId":"a-work","cursor":cursor})).await;
            worker = page["workers"].as_array().unwrap()[0].clone();
        }
        assert_eq!((tools, notes), (450, 450));
        messages.sort();
        let before = messages.len();
        messages.dedup();
        assert_eq!((before, messages.len()), (450, 450));
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Accept itself (not only its presentation) refuses a result whose check receipts exceed what one read presents.
    #[tokio::test]
    async fn accept_refuses_a_partial_receipt_set_with_a_readable_reason() {
        let (root, runtime, draft) = completed().await;
        let digest = crate::native_effects::observe(root.clone()).await.unwrap().project_digest.unwrap();
        mutate(&runtime, move |db, task, _| {
            for i in 0..1005 {
                lifecycle::event_tx(db, task, "NATIVE_COMMAND_FINISHED", json!({"receipt":{"revision":0,"argv":["/bin/echo",format!("c{i}")],"revision_digest":digest,
                    "tool_call_ref":format!("native-command:r{i}"),"observation":{"complete":true,"exit_code":0}}}))?;
            }
            Ok(())
        })
        .await;
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["accept"], false);
        let state = root.join(".cortex-dev/acceptance-state");
        std::fs::create_dir(&state).unwrap();
        let mut product = json!({"tasks":{},"sprints":[]});
        product["tasks"][&draft] = json!({"checkout":{"kind":"main"}});
        let p = json!({"action":"accept","taskId":draft,"digest":task["result"]["digest"],
            "version":task["result"]["sourceVersion"],"acknowledgeOpenChecks":false});
        let refused = accept(&root, "project", &draft, &state, &mut product, &p, &mut Git::new()).await.unwrap_err();
        assert!(refused.message.contains("check receipts"), "{}: {}", refused.code, refused.message);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Coordinator, workers and the Conversation each hold only their own rows. One session carries a
    /// coordinator and two workers; the coordinator page pages across more than 200 rows.
    #[tokio::test]
    async fn coordinator_and_worker_pages_hold_only_their_own_rows() {
        let (root, runtime, draft) = completed().await;
        mutate(&runtime, |db, task, session| {
            db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,assignment_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES ('a-coord',?,'a-coord','att-coord','orchestrator','ready','ready',1,'now','now')", [task])?;
            let mut seq = 0;
            let mut say = |db: &Connection, id: &str, text: &str| -> cortex_core::CoreResult<()> {
                seq += 1;
                db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,message_id,created_at) VALUES (?,?,'agent_message',?,?,'2026-11-17T10:00:00.000000+00:00')", (session, seq, text, id))?;
                Ok(())
            };
            for i in 0..250 {
                say(db, "att-coord:m", &format!("coordinator note {i}"))?;
                if i % 50 == 0 {
                    say(db, "att-a-work:m", &format!("work note {i}"))?;
                    say(db, "att-a-review:m", &format!("review note {i}"))?;
                }
            }
            for (n, call) in ["att-coord:c1", "att-a-work:c2", "att-a-review:c3"].into_iter().enumerate() {
                let update = json!({"sessionUpdate":"tool_call","toolCallId":call,"title":"Read a.ts","kind":"read","rawInput":{"path":"a.ts"},"status":"completed"});
                db.execute("INSERT INTO host_updates VALUES (?,?,?,'2026-11-17T10:00:01.000000+00:00')", (session, 10_000 + n as i64, update.to_string()))?;
            }
            Ok(())
        })
        .await;
        // The coordinator page: only coordinator rows, every page reachable by cursor.
        let mut texts = Vec::new();
        let mut cursor = Value::Null;
        let mut pages = 0;
        loop {
            let (d, c) = (draft.clone(), cursor.clone());
            let rows = runtime.host.executor.read(move |db| crate::machine::coordinator::read(db, "project", &json!({"taskIds":[d],"workerId":format!("coordinator:{d}"),"cursor":c})))
                .unwrap().await.unwrap().unwrap();
            let page = &rows[0];
            for a in page["activity"].as_array().unwrap() {
                texts.push(format!("{}|{}", a["text"].as_str().unwrap_or(""), a["toolCallRef"].as_str().unwrap_or("")));
            }
            pages += 1;
            assert!(pages < 10);
            match page["activityCursor"].as_str() {
                Some(next) => cursor = json!(next),
                None => break,
            }
        }
        assert!(texts.iter().filter(|t| t.contains("coordinator note")).count() == 250);
        assert!(texts.iter().all(|t| !t.contains("work note") && !t.contains("review note") && !t.contains("att-a-")));
        assert!(texts.iter().any(|t| t.ends_with("att-coord:c1")));
        assert!(pages >= 3);
        // Each worker page: its own narration and calls, nothing of the coordinator or the other worker.
        let facts = runtime.owner_facts("project", json!({"tasks":{},"sprints":[],"messages":[]}), json!({})).await.unwrap();
        for (id, mine, other) in [("a-work", "work note", "review note"), ("a-review", "review note", "work note")] {
            let worker = facts["workers"].as_array().unwrap().iter().find(|w| w["id"] == id).unwrap();
            let body = worker["activity"].to_string();
            assert!(body.contains(mine), "{id}");
            assert!(!body.contains(other) && !body.contains("coordinator note") && !body.contains("att-coord:"), "{id}");
            let other_attempt = if id == "a-work" { "att-a-review:" } else { "att-a-work:" };
            assert!(!body.contains(other_attempt), "{id}");
        }
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Owner store 2026-11-17: a worker that submitted a failed outcome (WORKER_FAILED with a
    /// summary and report) showed "reported no result". Its failed result, summary and report
    /// are on its card; workers without a result say why they ended, from recorded facts.
    #[tokio::test]
    async fn a_failed_outcome_and_other_endings_are_on_the_worker_card() {
        let (root, runtime, _) = completed().await;
        let set = |sql: &'static str, kind: Option<(&'static str, Value)>| {
            runtime
                .host
                .executor
                .publication(move |store| {
                    store.write(|db| {
                        let task: String = db.query_row("SELECT task_id FROM delegation_provisioning WHERE idempotency_key='a-work'", [], |r| r.get(0))?;
                        db.execute(sql, [])?;
                        if let Some((kind, payload)) = &kind {
                            lifecycle::event_tx(db, &task, kind, payload.clone())?;
                        }
                        Ok(())
                    })?;
                    Ok(())
                })
                .unwrap()
        };
        set(
            "UPDATE runtime_nodes SET status='failed', result_json='{\"assignment_outcome\":\"failed\",\"checks\":[],\"report_ref\":\"r_a408f1d4976c\",\"summary\":\"The documentation update remains incomplete.\"}' WHERE assignment_ref='a-work'",
            None,
        )
        .await
        .unwrap()
        .unwrap();
        set(
            "UPDATE runtime_nodes SET status='cancelled' WHERE assignment_ref='a-review'",
            Some(("WORKER_STOPPED_BY_OWNER", json!({"assignment_ref":"a-review","reason":"stop"}))),
        )
        .await
        .unwrap()
        .unwrap();
        let facts = || runtime.owner_facts("project", json!({"tasks":{},"sprints":[],"messages":[]}), json!({}));
        let all = facts().await.unwrap();
        let workers = all["workers"].as_array().unwrap();
        let work = workers.iter().find(|w| w["role"] == "backend-dev").unwrap();
        assert_eq!(work["status"], "failed");
        assert_eq!(work["result"]["outcome"], "failed");
        assert_eq!(work["result"]["reportId"], "r_a408f1d4976c");
        assert_eq!(work["result"]["summary"], "The documentation update remains incomplete.");
        assert_eq!(work["result"]["checks"], json!([]));
        assert!(work.get("endedReason").is_none());
        let review = workers.iter().find(|w| w["role"] == "code-reviewer").unwrap();
        assert!(review["result"].is_null());
        assert_eq!(review["endedReason"], "stopped_by_owner");
        // Without the owner-stop event the same cancelled node is the coordinator's doing.
        set("DELETE FROM runtime_events WHERE type='WORKER_STOPPED_BY_OWNER'", None).await.unwrap().unwrap();
        let all = facts().await.unwrap();
        let review = all["workers"].as_array().unwrap().iter().find(|w| w["role"] == "code-reviewer").unwrap().clone();
        assert_eq!(review["endedReason"], "cancelled_by_coordinator");
        // A failed worker whose retry records a lost session.
        set(
            "UPDATE runtime_nodes SET status='failed' WHERE assignment_ref='a-review'",
            Some(("ASSIGNMENT_RETRIED", json!({"previous_assignment":"a-review","failure_class":"worker_lost","observed_session":"absent","retry":1}))),
        )
        .await
        .unwrap()
        .unwrap();
        let all = facts().await.unwrap();
        let review = all["workers"].as_array().unwrap().iter().find(|w| w["role"] == "code-reviewer").unwrap().clone();
        assert_eq!(review["endedReason"], "worker_lost");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A result with no recorded workspace digest is not a claim about the files:
    /// it must not read as stale, and Accept still asks for verification first.
    #[tokio::test]
    async fn result_without_verified_digest_is_not_called_stale() {
        let (root, runtime, draft) = completed().await;
        runtime
            .host
            .executor
            .publication(|store| {
                store.write(|db| {
                    db.execute("DROP TRIGGER IF EXISTS runtime_events_no_delete", [])?;
                    db.execute("DELETE FROM runtime_events WHERE type='VERIFICATION_PASSED'", [])?;
                    Ok(())
                })?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let task = presented(&runtime, &draft).await;
        assert!(task["result"]["digest"].as_str().unwrap().starts_with("result:"), "{task}");
        assert_eq!(task["workspaceDigest"], task["result"]["digest"], "{task}");
        assert_eq!(task["reviewActions"]["accept"], false);
        assert_eq!(task["reviewActions"]["unblockAction"], "rerun_checks");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Live run: right after Request changes the Changes tab showed "Result v2" with the
    /// accepted result's check and an Accept v2 button while the task was working. A
    /// reopened task presents no result (and so no Accept) until it publishes a new one.
    #[tokio::test]
    async fn a_reopened_working_task_presents_no_result_until_a_new_one_exists() {
        let (root, runtime, draft) = completed().await;
        assert!(presented(&runtime, &draft).await["result"].is_object());
        runtime
            .host
            .executor
            .publication(|store| {
                store.write(|db| {
                    let task: String = db.query_row("SELECT task_id FROM runtime_tasks LIMIT 1", [], |r| r.get(0))?;
                    lifecycle::event_tx(db, &task, "TASK_REOPENED", json!({"previous":"COMPLETED","revision":0}))?;
                    db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?", [&task])?;
                    Ok(())
                })?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let task = presented(&runtime, &draft).await;
        assert!(task["result"].is_null(), "{task}");
        assert!(task["reviewActions"].is_null(), "{task}");
        // The next run's own host check is that run's, presented again.
        runtime
            .host
            .executor
            .publication(|store| {
                store.write(|db| {
                    let task: String = db.query_row("SELECT task_id FROM runtime_tasks LIMIT 1", [], |r| r.get(0))?;
                    lifecycle::event_tx(db, &task, "NATIVE_COMMAND_FINISHED", json!({"receipt":{"revision":0,"argv":["/bin/false"],"revision_digest":"next",
                        "tool_call_ref":"native-command:next","observation":{"complete":true,"exit_code":1}}}))?;
                    db.execute("UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id=?", [&task])?;
                    Ok(())
                })?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let next = presented(&runtime, &draft).await;
        assert_eq!(next["result"]["checks"].as_array().unwrap().len(), 1, "{next}");
        assert_eq!(next["result"]["checks"][0]["name"], "/bin/false");
        assert_eq!(next["result"]["acceptanceAvailable"], false);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Runs `body` on the fixture's one runtime task inside a store write.
    async fn with_task(
        runtime: &Runtime,
        body: impl FnOnce(&rusqlite::Connection, &str) -> cortex_core::CoreResult<()> + Send + 'static,
    ) {
        runtime
            .host
            .executor
            .publication(move |store| {
                store.write(|db| {
                    let task: String = db.query_row("SELECT task_id FROM runtime_tasks LIMIT 1", [], |r| r.get(0))?;
                    body(db, &task)
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }
    async fn presented_with(runtime: &Runtime, draft: &str, record: Value) -> Value {
        let mut product = json!({"tasks":{},"sprints":[],"messages":[]});
        product["tasks"][draft] = record;
        let facts = runtime.owner_facts("project", product, json!({})).await.unwrap();
        facts["product"]["tasks"][draft].clone()
    }
    /// Replaces the worker's and reviewer's reported checks of the fixture result.
    async fn report_checks(runtime: &Runtime, work: Value, review: Value) {
        with_task(runtime, move |db, task| {
            for (stage, checks) in [("execution", work), ("review", review)] {
                db.execute("UPDATE runtime_nodes SET result_json=json_set(result_json,'$.checks',json(?)) WHERE task_id=? AND stage=?",
                    (checks.to_string(), task, stage))?;
            }
            Ok(())
        })
        .await;
    }

    /// Live run (MCP task): runtime verification of the current version passed and the
    /// reviewer approved, yet Accept stayed off ("Run the unpassed checks first") because
    /// of the worker's own superseded claims (a run before its last edit, a trivial
    /// command, a check its sandbox could not run) and the reviewer's matching gaps. They
    /// are notes now; nothing needs acknowledging and Accept takes the result.
    #[tokio::test]
    async fn superseded_self_reported_checks_are_notes_once_runtime_verification_passed() {
        let (root, runtime, draft) = completed().await;
        report_checks(&runtime,
            json!([
                {"name":"test exits 0","outcome":"unverified","claimed_outcome":"passed","evidence_gap":"revision_mismatch",
                 "blocker":"Runtime kept this passed claim unverified: revision_mismatch.","tool_call_ref":"native-command:early"},
                {"name":"line count","outcome":"unverified","claimed_outcome":"passed","evidence_gap":"trivial_command",
                 "blocker":"Runtime kept this passed claim unverified: trivial_command.","tool_call_ref":"native-command:wc"},
                {"name":"npm test","outcome":"not_run","command":["npm","test"],"blocker":"npm is not installed in the guarded sandbox."}
            ]),
            json!([
                {"name":"/bin/true","outcome":"unverified","command":["/bin/true"],
                 "blocker":"The implementation report's native command receipt was rejected by the runtime for revision_mismatch."}
            ])).await;
        let task = presented(&runtime, &draft).await;
        let actions = &task["reviewActions"];
        assert_eq!(actions["accept"], true, "{task}");
        assert_eq!(actions["acknowledgeOpenChecks"], false, "{task}");
        assert!(actions["unblockAction"].is_null(), "{task}");
        assert_eq!(task["result"]["acceptanceAvailable"], true);
        let notes: Vec<&str> = task["result"]["warnings"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(notes.iter().any(|n| n.starts_with("The runtime's own verification of this version passed. 4 self-reported checks")), "{notes:?}");
        assert!(notes.contains(&"Worker check \u{201c}test exits 0\u{201d}: it ran before the final edits, so that run does not count."), "{notes:?}");
        assert!(notes.contains(&"Worker check \u{201c}npm test\u{201d}: not run: npm is not installed in the guarded sandbox."), "{notes:?}");
        assert!(notes.iter().any(|n| n.starts_with("Reviewer check \u{201c}/bin/true\u{201d}")), "{notes:?}");
        // The same-model review note is still there.
        assert!(notes.contains(&"The reviewer runs on the same model as the work it reviews."), "{notes:?}");
        let state = root.join(".cortex-dev/acceptance-state");
        std::fs::create_dir(&state).unwrap();
        let mut product = json!({"tasks":{},"sprints":[]});
        product["tasks"][&draft] = json!({"checkout":{"kind":"main"}});
        let p = json!({"action":"accept","taskId":draft,"digest":task["result"]["digest"],
            "version":task["result"]["sourceVersion"],"acknowledgeOpenChecks":false});
        let accepted = accept(&root, "project", &draft, &state, &mut product, &p, &mut Git::new())
            .await
            .unwrap_or_else(|e| panic!("{}: {}", e.code, e.message));
        assert_eq!(accepted["done"], true);
        assert_eq!(product["tasks"][&draft]["acceptance"]["open"], json!({"failed":0,"not_run":0,"unverified":0}));
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A failed self-reported check is never softened into a note, and without a passing
    /// runtime verification of this version an unconfirmed claim still blocks Accept.
    #[tokio::test]
    async fn failed_claims_and_unverified_runtime_checks_still_block_acceptance() {
        let (root, runtime, draft) = completed().await;
        report_checks(&runtime,
            json!([{"name":"unit tests","outcome":"failed","command":["/bin/false"],"tool_call_ref":"native-command:f"}]),
            json!([])).await;
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["accept"], false, "{task}");
        assert_eq!(task["reviewActions"]["acknowledgeOpenChecks"], true, "{task}");
        assert!(task["result"]["warnings"].as_array().unwrap().iter().all(|n| !n.as_str().unwrap().contains("self-reported")), "{task}");
        // The runtime's own verification failed: the unconfirmed claim stays an open check.
        report_checks(&runtime,
            json!([{"name":"npm test","outcome":"not_run","command":["npm","test"],"blocker":"npm is missing."}]),
            json!([])).await;
        with_task(&runtime, |db, task| {
            let digest: String = db.query_row("SELECT json_extract(payload_json,'$.revision_digest') FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'", [task], |r| r.get(0))?;
            lifecycle::event_tx(db, task, "VERIFICATION_FAILED", json!({"revision":0,"revision_digest":digest,
                "checks":[{"argv":["/bin/true"],"complete":true,"exit_code":1,"revision_digest":digest}]}))?;
            Ok(())
        }).await;
        let failed = presented(&runtime, &draft).await;
        assert_eq!(failed["reviewActions"]["accept"], false, "{failed}");
        assert!(failed["result"]["warnings"].as_array().map_or(true, |a| a.iter().all(|n| !n.as_str().unwrap().contains("self-reported"))), "{failed}");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Live run (Automatic sprint on the project folder): every earlier task's result went
    /// stale as soon as a later task wrote the folder. A project-folder sprint task is
    /// compared on the files it recorded writing; a change to one of them is still stale.
    #[tokio::test]
    async fn a_project_folder_sprint_result_is_stale_only_when_its_own_files_change() {
        let (root, runtime, draft) = completed().await;
        let written = format!("{:x}", Sha256::digest(b"after"));
        with_task(&runtime, move |db, task| {
            // The worker's last recorded write holds what is in work.txt; the runtime then verified.
            lifecycle::event_tx(db, task, "NATIVE_PROJECT_WRITTEN", json!({"channel_id":"ch-work","path":"work.txt","sha256":written,"add":1,"del":1}))?;
            let payload: String = db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'", [task], |r| r.get(0))?;
            lifecycle::event_tx(db, task, "VERIFICATION_PASSED", serde_json::from_str(&payload)?)?;
            Ok(())
        }).await;
        let sprint = json!({"checkout":{"kind":"main"},"sprintId":"sp_1"});
        assert_eq!(presented_with(&runtime, &draft, sprint.clone()).await["reviewActions"]["accept"], true);
        // A later task of the sprint writes another file of the same folder.
        std::fs::write(root.join("later.txt"), "a later task's file").unwrap();
        let task = presented_with(&runtime, &draft, sprint.clone()).await;
        assert_eq!(task["reviewActions"]["accept"], true, "{task}");
        assert_eq!(task["workspaceDigest"], task["result"]["digest"], "{task}");
        assert!(task["reviewActions"]["workspaceChanged"].is_null() && task["reviewActions"]["ownWritesOnly"].is_null(), "{task}");
        // Accept binds the same result identity.
        let state = root.join(".cortex-dev/acceptance-state");
        std::fs::create_dir(&state).unwrap();
        let mut product = json!({"tasks":{},"sprints":[]});
        product["tasks"][&draft] = sprint.clone();
        let p = json!({"action":"accept","taskId":draft,"digest":task["result"]["digest"],
            "version":task["result"]["sourceVersion"],"acknowledgeOpenChecks":false});
        accept(&root, "project", &draft, &state, &mut product, &p, &mut Git::new())
            .await
            .unwrap_or_else(|e| panic!("{}: {}", e.code, e.message));
        // Outside a sprint the whole folder still counts.
        let single = presented_with(&runtime, &draft, json!({"checkout":{"kind":"main"}})).await;
        assert_eq!(single["reviewActions"]["accept"], false, "{single}");
        assert_eq!(single["reviewActions"]["workspaceChanged"], true);
        // A change to a file the task wrote makes it stale again.
        std::fs::write(root.join("work.txt"), "changed by a later task").unwrap();
        let changed = presented_with(&runtime, &draft, sprint).await;
        assert_eq!(changed["reviewActions"]["accept"], false, "{changed}");
        assert_eq!(changed["reviewActions"]["workspaceChanged"], true);
        assert_ne!(changed["workspaceDigest"], changed["result"]["digest"]);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Live run: after Accept and the app's own "Commit to main" of exactly that result,
    /// its card still said "Stale result" (the commit moved HEAD, which the folder digest
    /// covers). The committed result is not stale.
    #[tokio::test]
    async fn the_apps_own_commit_of_a_result_does_not_make_it_stale() {
        let (root, runtime, draft) = completed().await;
        let digest = presented(&runtime, &draft).await["result"]["digest"].clone();
        std::fs::write(root.join("moved-head.txt"), "stands in for the commit").unwrap();
        let committed = json!({"checkout":{"kind":"main"},"status":"done","work":"merged",
            "integration":{"method":"commit","digest":digest,"targetBranch":"main"}});
        let task = presented_with(&runtime, &draft, committed.clone()).await;
        assert_eq!(task["workspaceDigest"], digest, "{task}");
        // Another integration identity is not this result's commit.
        let mut other = committed;
        other["integration"]["digest"] = json!("f".repeat(64));
        assert_ne!(presented_with(&runtime, &draft, other).await["workspaceDigest"], digest);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Live run: after Request changes the Inspector counted the previous version's
    /// worker and reviewer claims as the current checks. Those workers are superseded.
    #[tokio::test]
    async fn workers_of_a_reopened_version_are_marked_superseded() {
        let (root, runtime, _draft) = completed().await;
        let workers = |facts: &Value| facts["workers"].as_array().unwrap().clone();
        let facts = runtime.owner_facts("project", json!({"tasks":{},"sprints":[],"messages":[]}), json!({})).await.unwrap();
        assert!(workers(&facts).iter().all(|w| w.get("superseded").is_none()), "{facts}");
        with_task(&runtime, |db, task| {
            lifecycle::event_tx(db, task, "TASK_REOPENED", json!({"previous":"COMPLETED","revision":0}))?;
            db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?", [task])?;
            Ok(())
        }).await;
        let facts = runtime.owner_facts("project", json!({"tasks":{},"sprints":[],"messages":[]}), json!({})).await.unwrap();
        assert_eq!(workers(&facts).len(), 2);
        assert!(workers(&facts).iter().all(|w| w["superseded"] == true), "{facts}");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Live run: a worker that only read files showed "No activity recorded yet" on its
    /// page while the task timeline showed its reads. Its page lists the same retained
    /// tool rows: reads, skill reads and searches, but not its Cortex writes and commands
    /// twice, and never another worker's steps.
    #[tokio::test]
    async fn a_workers_page_lists_its_reads_and_other_tool_steps_from_the_timeline_rows() {
        let (root, runtime, _draft) = completed().await;
        with_task(&runtime, |db, _task| {
            let session: String = db.query_row("SELECT acp_session_id FROM workspace_executions LIMIT 1", [], |r| r.get(0))?;
            let rows = [
                json!({"sessionUpdate":"tool_call","toolCallId":"att-a-work:read","status":"in_progress","title":"worker-backend-dev: mcp.cortex.project_read","kind":"execute",
                    "rawInput":{"server":"cortex","tool":"project_read","arguments":{"path":"src/counter.js"}}}),
                json!({"sessionUpdate":"tool_call_update","toolCallId":"att-a-work:read","status":"completed"}),
                json!({"sessionUpdate":"tool_call","toolCallId":"att-a-work:skill","status":"in_progress","title":"worker-backend-dev: mcp.cortex.read_skill","kind":"execute",
                    "rawInput":{"server":"cortex","tool":"read_skill","arguments":{"skill":"documentation-sync"}}}),
                json!({"sessionUpdate":"tool_call","toolCallId":"att-a-work:grep","status":"completed","title":"worker-backend-dev: grep TODO","kind":"search","rawInput":{"pattern":"TODO","path":"src"}}),
                json!({"sessionUpdate":"tool_call","toolCallId":"att-a-work:edit","status":"completed","title":"worker-backend-dev: mcp__cortex__project_edit","kind":"other","rawInput":{"path":"work.txt"}}),
                json!({"sessionUpdate":"tool_call","toolCallId":"att-a-review:read","status":"completed","title":"worker-code-reviewer: mcp__cortex__project_read","kind":"other","rawInput":{"path":"work.txt"}}),
            ];
            for (seq, row) in rows.iter().enumerate() {
                db.execute("INSERT INTO host_updates VALUES (?,?,?,?)", rusqlite::params![&session, 1000 + seq as i64, row.to_string(), cortex_core::now()])?;
            }
            Ok(())
        }).await;
        let facts = runtime.owner_facts("project", json!({"tasks":{},"sprints":[],"messages":[]}), json!({})).await.unwrap();
        let workers = facts["workers"].as_array().unwrap();
        let work = workers.iter().find(|w| w["role"] == "backend-dev").unwrap();
        let steps: Vec<&Value> = work["activity"].as_array().unwrap().iter().filter(|a| a["kind"] == "tool").collect();
        let step = |call: &str| steps.iter().find(|a| a["toolCallRef"] == format!("att-a-work:{call}")).map(|a| serde_json::from_str::<Value>(a["args"].as_str().unwrap()).unwrap());
        let read = step("read").expect("the read is on the worker's page");
        assert_eq!((read["path"].as_str(), read["status"].as_str()), (Some("src/counter.js"), Some("completed")));
        assert_eq!(step("skill").unwrap()["ref"], "documentation-sync");
        assert_eq!(step("grep").unwrap()["query"], "TODO");
        assert!(step("edit").is_none(), "a Cortex write is the edit row above, not a second step");
        assert!(steps.iter().all(|a| a["text"].as_str().unwrap().starts_with("worker-backend-dev:")));
        // Every row has a time and a distinct sequence, and they are in time order.
        let all = work["activity"].as_array().unwrap();
        assert!(all.iter().all(|a| a["at"].is_i64()), "{all:?}");
        let mut sequences: Vec<i64> = all.iter().map(|a| a["sequence"].as_i64().unwrap()).collect();
        assert!(all.windows(2).all(|w| w[0]["at"].as_i64() <= w[1]["at"].as_i64()));
        sequences.sort();
        sequences.dedup();
        assert_eq!(sequences.len(), all.len());
        let review = workers.iter().find(|w| w["role"] == "code-reviewer").unwrap();
        let reviewer: Vec<&Value> = review["activity"].as_array().unwrap().iter().filter(|a| a["kind"] == "tool").collect();
        assert_eq!(reviewer.len(), 1, "{review}");
        assert_eq!(reviewer[0]["toolCallRef"], "att-a-review:read");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
