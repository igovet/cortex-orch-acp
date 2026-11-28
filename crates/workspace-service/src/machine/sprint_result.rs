//! Published evidence of a member of the authenticated Workspace sprint.
use super::{redact_display_text, sprint_graph::Scope, task_facts_git};
use cortex_core::{store::documents::Publisher, CoreError, CoreResult, Store};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

/// A historical verification observation remains tied to the plan revision
/// on which it ran. Never export command output or turn a missing run into a pass.
fn outcome(db: &Connection, task: &str) -> CoreResult<Value> {
    let (state, revision, synthesis, blocker): (String, i64, Option<String>, Option<String>) = db
        .query_row(
        "SELECT state,revision,synthesis,blocker FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let last: Option<(i64, String, String, String)> = db
        .query_row(
            "SELECT id,type,actor,payload_json FROM runtime_events WHERE task_id=?1 AND type IN
         ('VERIFICATION_PASSED','VERIFICATION_FAILED','VERIFICATION_REVISED')
         AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type IN
             ('TASK_REOPENED','PLAN_ACCEPTED','REVIEW_STARTED')),0)
         ORDER BY id DESC LIMIT 1",
            [task],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let verification = match last {
        Some((_, _, actor, _)) if actor != "runtime" => {
            json!({"last_observation":null,"reason":"verification_evidence_unconfirmed"})
        }
        Some((event, kind, _, raw)) => {
            let payload: Value = serde_json::from_str(&raw)?;
            if kind == "VERIFICATION_REVISED" && payload["accepted_unverified"].is_null() {
                json!({"last_observation":null,"reason":"verification_procedure_revised"})
            } else if kind == "VERIFICATION_REVISED"
                && cortex_core::lifecycle::completion::retained_verification_waiver(
                    db, task, event, &payload,
                )?
            {
                let accepted: Vec<_> = payload["accepted_unverified"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|argv| argv.get(0))
                    .collect();
                json!({"last_observation":null,"reason":"owner_accepted_unverified","programs":accepted})
            } else if kind == "VERIFICATION_REVISED" {
                json!({"last_observation":null,"reason":"verification_evidence_unconfirmed"})
            } else if kind == "VERIFICATION_PASSED"
                && payload["revision"] == revision
                && !cortex_core::lifecycle::completion::retained_verification_pass(
                    db, task, event, &payload,
                )?
            {
                json!({"last_observation":null,"reason":"verification_evidence_unconfirmed"})
            } else if payload["revision"] == revision {
                let checks:Vec<_>=payload["checks"].as_array().into_iter().flatten().map(|v| json!({
                    "receipt_ref":v["receipt_ref"],"program":v["argv"].get(0),"complete":v["complete"],"exit_code":v["exit_code"]
                })).collect();
                json!({"last_observation":kind,"revision":revision,"revision_digest":payload["revision_digest"],
                    "review_event":payload["review_event"],"checks":checks})
            } else {
                json!({"last_observation":null,"reason":"no_verification_for_this_revision"})
            }
        }
        None => json!({"last_observation":null,"reason":"no_verification_for_this_revision"}),
    };
    let constraints = json!({"write_denials":cortex_core::lifecycle::steering::denials(db,task)?,
        "access_mode":cortex_core::access::current(db,task)?});
    let owner_resolution: Option<String> = db
        .query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=?1
         AND type IN ('OWNER_TASK_RESOLVED','USER_FORCE_FINISHED')
         AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1
             AND type='TASK_REOPENED'),0) ORDER BY id DESC LIMIT 1",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    let owner_resolution = owner_resolution
        .map(|raw| serde_json::from_str::<Value>(&raw))
        .transpose()?
        .map(|v| {
            json!({"disposition":v["disposition"],"certified":v["certified"],
            "processes_unconfirmed":v["processes_unconfirmed"]})
        });
    Ok(
        json!({"state":state,"revision":revision,"synthesis":synthesis,"blocker":blocker,"verification":verification,
        "owner_resolution":owner_resolution,"constraints":constraints,"decisions":decisions(db,task,None,16)?}),
    )
}

/// Report publication and use by an assignment are separate facts. Historical
/// reports remain readable, but cannot stand in for the current result.
fn report_use(db: &Connection, task: &str, report: &str, digest: &str) -> CoreResult<&'static str> {
    let (current, bindings): (bool, i64) = db.query_row(
        "SELECT coalesce(max(n.revision=t.revision AND n.revision>coalesce(
             (SELECT json_extract(e.payload_json,'$.revision') FROM runtime_events e
              WHERE e.task_id=t.task_id AND e.type='TASK_REOPENED' ORDER BY e.id DESC LIMIT 1),-1)),0),
             count(*)
         FROM runtime_nodes n JOIN runtime_tasks t ON t.task_id=n.task_id
         JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
         WHERE n.task_id=? AND n.status='completed'
         AND json_extract(n.result_json,'$.report_ref')=?
         AND json_extract(n.result_json,'$.report_digest')=?
         AND 1=(SELECT count(*) FROM runtime_events e
                WHERE e.task_id=n.task_id AND e.type='WORKER_COMPLETED' AND e.actor='runtime'
                AND json_extract(e.payload_json,'$.assignment_ref')=n.assignment_ref
                AND json_extract(e.payload_json,'$.attempt_id')=p.attempt_id
                AND json_extract(e.payload_json,'$.result.report_ref')=json_extract(n.result_json,'$.report_ref')
                AND json_extract(e.payload_json,'$.result.report_digest')=json_extract(n.result_json,'$.report_digest'))",
        (task, report, digest),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(if bindings != 1 {
        "unbound"
    } else if current {
        "current_revision"
    } else {
        "historical"
    })
}

/// Question IDs and answers are evidence from the predecessor, not authority
/// for a new task. The cursor is confined to its runtime task and record type.
fn decisions(db: &Connection, task: &str, cursor: Option<&str>, limit: usize) -> CoreResult<Value> {
    let prefix = format!("sd.{task}.");
    let before = match cursor {
        Some(cursor) => cursor
            .strip_prefix(&prefix)
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|v| *v > 0)
            .ok_or(CoreError::Refused("invalid_cursor"))?,
        None => i64::MAX,
    };
    let mut statement = db.prepare(
        "SELECT rowid,id,text,answer_text FROM pending_questions
        WHERE task_id=? AND rowid<? AND status='answered' AND answer_text IS NOT NULL
        AND dedup_key NOT GLOB 'native_permission:*' ORDER BY rowid DESC LIMIT ?",
    )?;
    let rows = statement
        .query_map((task, before, (limit + 1) as i64), |r| {
            let question: String = r.get(2)?;
            let answer: String = r.get(3)?;
            Ok((
                r.get::<_, i64>(0)?,
                json!({"question_id":r.get::<_,String>(1)?,
            "question":redact_display_text(&question),"answer":redact_display_text(&answer)}),
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let next = (rows.len() > limit).then(|| format!("{prefix}{}", rows[limit - 1].0));
    let items: Vec<_> = rows
        .into_iter()
        .take(limit)
        .map(|(_, question)| question)
        .collect();
    Ok(json!({"items":items,"next_cursor":next}))
}

pub(crate) fn read(
    store: &Store,
    scope: &Scope,
    args: &Value,
    recipient: &std::path::Path,
) -> CoreResult<Value> {
    let id = args["member_id"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_arguments"))?;
    let member = scope.member(id)?;
    let cursor = args["cursor"].as_str();
    let limit = args["limit"].as_u64();
    let publisher = Publisher::new(store);
    if let Some(report) = args["report_id"].as_str() {
        let task = member
            .runtime
            .as_deref()
            .ok_or(CoreError::Refused("report_not_available"))?;
        let page = publisher.immutable_report_page(
            task,
            report,
            cursor,
            limit.unwrap_or(4000) as usize,
        )?;
        let usage = store.read(|db| report_use(db, task, report, &page.sha256))?;
        return Ok(
            json!({"member_id":id,"report":{"report_id":report,"markdown":page.markdown,"sha256":page.sha256,
            "result_binding":usage,"total_characters":page.total_characters,"next_cursor":page.next_cursor}}),
        );
    }
    let mut value =
        json!({"member_id":id,"task":member.record,"closure":member.record["closedAs"]});
    let Some(task) = member.runtime.as_deref() else {
        if cursor.is_some() {
            return Err(CoreError::Refused("invalid_cursor"));
        }
        value["outcome"] = json!({"status":"unavailable","reason":"task_has_no_runtime_history"});
        value["code"] = json!({"status":"unknown","scope":"recorded_paths"});
        value["reports"] = json!({"items":[],"next_cursor":null});
        return Ok(value);
    };
    if cursor.is_some_and(|v| v.starts_with("sd.")) {
        let page = store.read(|db| decisions(db, task, cursor, limit.unwrap_or(16) as usize))?;
        return Ok(json!({"member_id":id,"decisions":page}));
    }
    let catalogue = publisher.immutable_reports(task, cursor, limit.unwrap_or(25) as usize)?;
    let (items, result, code) = store.read(|db| {
        let items = catalogue.reports.iter().map(|r| Ok(json!({
            "report_id":r.report_id,"title":r.title,"summary":r.summary,"sha256":r.sha256,"size_bytes":r.size_bytes,
            "result_binding":report_use(db, task, &r.report_id, &r.sha256)?
        }))).collect::<CoreResult<Vec<_>>>()?;
        Ok((
            items,
            outcome(db, task)?,
            task_facts_git::sprint_availability(db, id, recipient)?,
        ))
    })?;
    value["reports"] = json!({"items":items,"next_cursor":catalogue.next_cursor});
    value["outcome"] = result;
    value["code"] = code;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::{
        channels::{self, Principal},
        lifecycle::{self, completion, FoundationGuards, TaskMachine},
        ports::{CommandObservation, ProcessObservation},
        store::documents::Publication,
        OpenMode, TaskState,
    };

    #[tokio::test]
    async fn downstream_coordinator_reads_bound_predecessor_pages_and_detects_stale_or_missing_evidence(
    ) {
        let root =
            std::env::temp_dir().join(format!("sprint-handoff-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(
            crate::native_runtime::key(&root).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let machine = TaskMachine::new(&store);
        let owner_a = machine
            .create("run-a", "Describe the API", "owner")
            .unwrap();
        let owner = machine.create("run-b", "Use the API", "owner").unwrap();
        machine
            .transition(
                "run-b",
                &owner,
                "planning",
                TaskState::Planning,
                None,
                &FoundationGuards,
            )
            .unwrap();
        let project = "sprint_fixture";
        let state = root.join("state");
        let product = super::super::native::product_path(&state, project).unwrap();
        std::fs::create_dir_all(product.parent().unwrap()).unwrap();
        std::fs::write(product, json!({
            "tasks": {
                "card-a":{"sprintId":"sprint-a","status":"done","closedAs":"as_is","notVerifiedByCortex":true,"dependencies":[]},
                "card-b":{"sprintId":"sprint-a","dependencies":["card-a"]}
            },
            "sprints":[{"id":"sprint-a","projectId":project,"taskIds":["card-a","card-b"]}]
        }).to_string()).unwrap();
        let publisher = Publisher::new(&store);
        let draft = publisher
            .create_draft("run-a", "author", "general", "draft")
            .unwrap();
        let body = format!(
            "{}\n\n{}\nFinal input key: record_id\n",
            draft.markdown.lines().next().unwrap(),
            "API input schema. ".repeat(400)
        );
        publisher
            .write_draft("run-a", "author", &draft.draft_id, &body)
            .unwrap();
        let report = publisher
            .publish(
                "run-a",
                "author",
                &draft.draft_id,
                "publish",
                Publication {
                    title: "API contract",
                    summary: "The downstream input schema",
                    author: "author",
                    source_revision: 0,
                    artifacts: json!([]),
                },
            )
            .unwrap();
        std::fs::write(root.join("contract.txt"), "record_id\n").unwrap();
        let mut snapshot = crate::native_effects::observe(root.clone()).await.unwrap();
        let argv = vec![
            "/usr/bin/test".to_owned(),
            "-s".to_owned(),
            "contract.txt".to_owned(),
        ];
        let exit = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(exit.success());
        let credential = store.write(|db| {
            db.execute("INSERT INTO workspace_projects(project_id,project_root,activated_by,activated_at) VALUES (?,?,'owner','now')",
                (project, root.to_str().unwrap()))?;
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at)
                VALUES ('outside',?,'Other project task','now')", [project])?;
            for (card, run) in [("card-a","run-a"),("card-b","run-b")] {
                db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,execution_id,created_at) VALUES (?,?,?,?,'now')",
                    (card,project,card,run))?;
                db.execute("INSERT INTO acp_host_sessions(session_id,project_root,task_id,client_info,client_capabilities,mcp_servers,created_at)
                    VALUES (?,?,?,'{}','{}','[]','now')", (card,root.to_str().unwrap(),run))?;
                db.execute("INSERT INTO workspace_executions(execution_id,task_id,runtime_task_id,acp_session_id,owner_epoch,bound_at) VALUES (?,?,?,?,1,'now')",
                    (run,card,run,card))?;
            }
            db.execute("UPDATE runtime_tasks SET revision=1,state='PLAN_READY',plan_json=? WHERE task_id='run-a'",
                [json!({"verification":[argv],"outcomes":[{"id":"api","statement":"Publish the API contract"}]}).to_string()])?;
            completion::execution_baseline_tx(db, "run-a", &snapshot)?;
            db.execute("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status,assignment_ref)
                VALUES ('run-a',1,'describe','worker-explorer','Describe API','[]','[]','execution','running','assignment-a')", [])?;
            let worker = channels::issue_tx(db, "run-a", &owner_a, Principal::Worker {assignment:"assignment-a"})?;
            let attempt = channels::authenticate(db, &worker.channel_id, worker.secret(), false)?.attempt;
            db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
                VALUES ('run-a','worker_result','assignment-a','result-a',?,'runtime','now')",
                [json!({"report_ref":report.report_id}).to_string()])?;
            snapshot.artifacts.insert(report.report_id.clone(), report.sha256.clone());
            snapshot.workers.insert(attempt, ProcessObservation::ConfirmedExited);
            completion::record_result_tx(db, "run-a", "assignment-a",
                &json!({"report_ref":report.report_id,"summary":"Published contract","checks":[]}), &snapshot)?;
            completion::add_review_tx(db, "run-a", &snapshot)?;
            lifecycle::event_tx(db, "run-a", "VERIFICATION_PASSED", json!({"revision":1,"revision_digest":"original","checks":[]}))?;
            assert_eq!(outcome(db, "run-a")?["verification"]["reason"], "verification_evidence_unconfirmed");
            completion::retain_check_tx(db, "run-a", &owner_a, "fixture-check", &argv,
                &CommandObservation{complete:true,exit_code:exit.code(),receipt_ref:"check-contract-a".into()}, &snapshot)?;
            assert!(completion::record_verification_tx(db, "run-a", &["fixture-check".into()], &snapshot)?);
            lifecycle::reconciliation::submit_synthesis_tx(db, "run-a", &json!({"text":"See API contract",
                "outcomes":[{"id":"api","status":"done","evidence":report.report_id}]}))?;
            completion::complete_tx(db, "run-a", &snapshot)?;
            let c = channels::issue_tx(db, "run-b", &owner, Principal::Coordinator)?;
            channels::bind_session_tx(db, &c, "coordinator-b")?;
            Ok(c)
        }).unwrap();
        let identity = store
            .read(|db| {
                channels::authenticate(db, &credential.channel_id, credential.secret(), true)
            })
            .unwrap();
        let read = |args| {
            crate::native_context_tools::sprint_read(
                &store,
                &credential,
                &identity,
                Some(&state),
                &root,
                "sprint_task_result",
                &args,
            )
        };
        let catalogue = read(json!({"member_id":"card-a"})).unwrap();
        assert_eq!(catalogue["task"]["notVerifiedByCortex"], true);
        assert_eq!(
            catalogue["outcome"]["verification"]["last_observation"],
            "VERIFICATION_PASSED"
        );
        assert_eq!(
            catalogue["reports"]["items"][0]["report_id"],
            report.report_id
        );
        assert_eq!(
            catalogue["reports"]["items"][0]["result_binding"],
            "current_revision"
        );
        assert_eq!(catalogue["code"]["status"], "unknown");
        let args = json!({"member_id":"card-a", "report_id": report.report_id, "limit": 4000});
        let first = read(args.clone()).unwrap();
        let cursor = &first["report"]["next_cursor"];
        assert!(cursor.is_string());
        assert!(!first["report"]["markdown"]
            .as_str()
            .unwrap()
            .contains("record_id"));
        let second =
            read(json!({"member_id":"card-a", "report_id":report.report_id, "cursor":cursor}))
                .unwrap();
        assert!(second["report"]["next_cursor"].is_null());
        assert_eq!(first["report"]["sha256"], second["report"]["sha256"]);
        let consumed = second["report"]["markdown"]
            .as_str()
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("Final input key: "));
        assert_eq!(consumed, Some("record_id"));
        assert_eq!(
            format!(
                "{}{}",
                first["report"]["markdown"].as_str().unwrap(),
                second["report"]["markdown"].as_str().unwrap()
            ),
            body
        );
        assert_eq!(
            read(json!({"member_id":"outside"})).unwrap_err().code(),
            "task_outside_sprint"
        );
        assert!(
            read(json!({"member_id":"card-b","report_id":report.report_id,"cursor":cursor}))
                .is_err()
        );
        store
            .write(|db| {
                db.execute(
                    "UPDATE workspace_executions SET owner_epoch=2 WHERE task_id='card-a'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            read(args.clone()).unwrap_err().code(),
            "sprint_membership_invalid"
        );
        store
            .write(|db| {
                db.execute(
                    "UPDATE workspace_executions SET owner_epoch=1 WHERE task_id='card-a'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        store
            .write(|db| {
                lifecycle::event_tx(db, "run-a", "REVIEW_STARTED", json!({"revision":1}))?;
                Ok(())
            })
            .unwrap();
        assert!(
            read(json!({"member_id":"card-a"})).unwrap()["outcome"]["verification"]
                ["last_observation"]
                .is_null()
        );
        store
            .write(|db| {
                lifecycle::event_tx(db, "run-a", "TASK_REOPENED", json!({"revision":1}))?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            read(args.clone()).unwrap()["report"]["result_binding"],
            "historical"
        );
        let filename = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT filename FROM reports WHERE id=?",
                    [&report.report_id],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .unwrap();
        let path = store.key().directory().join("run-a").join(filename);
        std::fs::write(&path, b"partial").unwrap();
        assert_eq!(read(args.clone()).unwrap_err().code(), "file_conflict");
        std::fs::remove_file(path).unwrap();
        assert!(read(args).is_err());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn result_binds_checks_to_revision_and_pages_work_decisions_without_permission_details() {
        let root =
            std::env::temp_dir().join(format!("sprint-result-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(
            crate::native_runtime::key(&root).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        TaskMachine::new(&store)
            .create("a", "Investigate the API", "owner")
            .unwrap();
        store.write(|db|{
            db.execute("UPDATE runtime_tasks SET revision=2,synthesis='Recorded API decision' WHERE task_id='a'",[])?;
            cortex_core::lifecycle::event_tx(db,"a","VERIFICATION_PASSED",json!({"revision":1}))?;
            for i in 0..20 {
                let id=format!("q{i}");
                db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,status,answer_text,created_at) VALUES (?,'a',?,'API format?','answered','plain text','now')",(&id,&id))?;
            }
            db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,status,answer_text,created_at) VALUES ('secret','a','native_permission:fixture','private permission body','answered','Allow once','now')",[])?;
            Ok(())
        }).unwrap();
        let first = store.read(|db| outcome(db, "a")).unwrap();
        assert_eq!(
            first["verification"]["reason"],
            "no_verification_for_this_revision"
        );
        assert!(!first.to_string().contains("private permission body"));
        let cursor = first["decisions"]["next_cursor"].as_str().unwrap();
        let rest = store
            .read(|db| decisions(db, "a", Some(cursor), 16))
            .unwrap();
        assert_eq!(rest["items"].as_array().unwrap().len(), 4);
        assert!(rest["next_cursor"].is_null());
        assert!(store
            .read(|db| decisions(db, "another", Some(cursor), 16))
            .is_err());
        store.write(|db|{
            cortex_core::lifecycle::event_tx(db,"a","VERIFICATION_FAILED",json!({"revision":2,"checks":[{"argv":["test"],"exit_code":1,"complete":true,"stderr":"private diagnostic"}]}))?;
            Ok(())
        }).unwrap();
        let checked = store.read(|db| outcome(db, "a")).unwrap();
        assert_eq!(
            checked["verification"]["last_observation"],
            "VERIFICATION_FAILED"
        );
        assert_eq!(checked["verification"]["checks"][0]["exit_code"], 1);
        assert!(!checked.to_string().contains("private diagnostic"));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
