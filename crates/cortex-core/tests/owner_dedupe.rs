//! The owner is never asked the same thing twice, and the same review findings,
//! access request or rate limit never cycle. Event shapes are minimal copies of
//! real owner-store records (2026-11-21): Youtube t_f2c20 37345-37554 (declined
//! generated-file reads re-asked after every plan), t_e8b28e435551 16107-16213 (the
//! same question asked twice), Business t_d530de978253 (nine review-cycle ladders)
//! and AI-Frontier t_a1a28b962190 324515-324518 (a registry 429 routed as access).
mod support;
use cortex_core::{
    lifecycle::{self, domain, obstacles, review, skill_proposals, waits, TaskMachine},
    workspace::{self, NativeExecutionPolicy},
    Store, TaskState,
};
use serde_json::{json, Value};
use support::Project;

fn started() -> (Project, Store, String) {
    let p = Project::new();
    let s = p.open();
    s.write(|db| {
        db.execute(
            "INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)",
            [p.0.to_str()],
        )?;
        Ok(())
    })
    .unwrap();
    let draft = workspace::create_draft(
        &s,
        "p",
        "owner",
        &json!({
        "protocol":{"major":1,"minor":8},"requestId":"draft","projectId":"p","actorId":"owner",
        "operation":"tasks.create","target":"p","expectedVersion":"0","ownerEpoch":null,
        "payload":{"description":"Implement the change","checkout":{"kind":"main"}}}),
    )
    .unwrap()
    .receipt;
    let start = json!({"protocol":{"major":1,"minor":8},"requestId":"start","projectId":"p","actorId":"owner",
        "operation":"tasks.start","target":draft["target"],"expectedVersion":draft["entityVersion"],"ownerEpoch":null,"payload":{}});
    let receipt = workspace::start_native_input_with_policy(
        &s,
        "p",
        "owner",
        &start,
        None,
        None,
        Some(NativeExecutionPolicy::AutomaticContinuation),
    )
    .unwrap();
    let task = receipt["task"].as_str().unwrap().to_owned();
    (p, s, task)
}

fn count(s: &Store, task: &str, kind: &str) -> i64 {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=? AND type=?",
            (task, kind),
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

fn latest(s: &Store, task: &str, kind: &str) -> Value {
    s.read(|db| {
        let raw: String = db.query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type=? ORDER BY id DESC LIMIT 1",
            (task, kind),
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str(&raw)?)
    })
    .unwrap()
}

/// The newest `LOOP_OBSERVED` of one loop kind (the ladder records its own as well).
fn observed(s: &Store, task: &str, kind: &str) -> Value {
    s.read(|db| {
        let raw: String = db.query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='LOOP_OBSERVED'
             AND json_extract(payload_json,'$.kind')=? ORDER BY id DESC LIMIT 1",
            (task, kind),
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str(&raw)?)
    })
    .unwrap()
}

fn node_result(s: &Store, task: &str, assignment: &str) -> Value {
    s.read(|db| {
        let raw: String = db.query_row(
            "SELECT result_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
            (task, assignment),
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str(&raw)?)
    })
    .unwrap()
}

/// The owner's recorded answer to a coordinator question, as the answer route stores it.
fn answered(db: &rusqlite::Transaction<'_>, task: &str, id: &str, text: &str, answer: &str) {
    db.execute(
        "INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,answer_text,answer_source,created_at,answered_at)
         VALUES (?,?,?,?,'[\"Close as is\",\"Read every file\"]','answered',?,'text','now','now')",
        (id, task, format!("k-{id}"), text, answer),
    )
    .unwrap();
    lifecycle::event_tx(
        db,
        task,
        "NATIVE_QUESTION_ANSWERED",
        json!({"question_id":id,"answer_source":"text"}),
    )
    .unwrap();
    lifecycle::event_tx(
        db,
        task,
        "USER_ANSWER_RECEIVED",
        json!({"question_id":id,"answer_source":"text"}),
    )
    .unwrap();
}

const TWINS: &str = "channels/life-is-broken/graphics/characters/shopper-card-rack-wide.json, shopper-freezer-wide.json, shopper-queue-wide.json";

/// Youtube t_f2c20 37345 -> 37453: the decided item "three twins" came back split into
/// one item per file and with a reworded reason; it is a disclosed limit, not a question.
#[test]
fn a_reworded_answered_review_item_is_disclosed_not_asked() {
    let (_p, s, task) = started();
    s.write(|db| {
        lifecycle::event_tx(db, &task, "REVIEW_DEAD_END_ROUTED", json!({"review":"_review_1","revision":43,"verdict":"INCOMPLETE",
            "blocked":[{"blocker":"owner_decision","item":TWINS,
                "reason":"The owner declined a full read of generated files; work accepted in substance."}]}))?;
        answered(db, &task, "pq_33e0f872a56440018a65c3a98935d5e0", "How to close t08?",
            "Leave t08 formally incomplete and close it as accepted in substance");
        db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=44,plan_json='{\"tasks\":[]}' WHERE task_id=?", [&task])?;
        let result = json!({"verdict":"INCOMPLETE","report_ref":"r_6b194c772fbc","checks":[],"project_digest":"a".repeat(64),
            "not_covered":[
                {"item":"channels/life-is-broken/graphics/characters/shopper-card-rack-wide.json","blocker":"owner_decision",
                 "reason":"Declined by the owner. Substitute evidence: tests assert twin equivalence."},
                {"item":"channels/life-is-broken/graphics/characters/shopper-freezer-wide.json","blocker":"owner_decision",
                 "reason":"Same owner decision as above."},
                {"item":"assets/preview.png","blocker":"tool_unavailable","reason":"image"}]});
        db.execute("INSERT INTO runtime_nodes VALUES (?,44,'_review_1','worker-code-reviewer','Review','[]','[\"**\"]','review','completed','rv44',?)",
            (&task, result.to_string()))?;
        review::advance_tx(db, &task)?;
        assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Verifying);
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, &task, "REVIEW_LIMIT_DISCLOSED"), 1);
    assert_eq!(count(&s, &task, "OWNER_DECISION_REQUIRED"), 0);
    assert_eq!(count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
    let broken = latest(&s, &task, "LOOP_BROKEN");
    assert_eq!(
        (broken["kind"].as_str(), broken["action"].as_str()),
        (Some("owner_decision"), Some("disclosed_limit"))
    );
    assert!(broken["answer"]
        .as_str()
        .unwrap()
        .contains("accepted in substance"));
    let result = node_result(&s, &task, "rv44");
    let names: Vec<&str> = result["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("Review did not read channels/")),
        "{names:?}"
    );
    assert!(result["checks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["outcome"] == "unverified"));

    // A reason that cites an answered owner question is decided by that answer alone.
    let (_p, s, task) = started();
    s.write(|db| {
        answered(
            db,
            &task,
            "pq_643872de625f473c8e4c9c9a8c19aaf4",
            "Read the generated files?",
            "No, close as is",
        );
        assert_eq!(
            review::decided_owner_item_tx(
                db,
                &task,
                "channels/life-is-broken/tools/character-prep/shopper-art.mjs",
                "Owner answers pq_643872de625f473c8e4c9c9a8c19aaf4 declined a full read"
            )?
            .as_deref(),
            Some("No, close as is")
        );
        // An unrelated item without an answer is still the owner's to decide.
        assert_eq!(
            review::decided_owner_item_tx(db, &task, "docs/pricing.md", "pricing policy")?,
            None
        );
        Ok(())
    })
    .unwrap();
}

/// Youtube t_f2c20 37453 -> 37538: the same decision in a new revision, through the
/// stop route, is applied, never another OWNER_DECISION_REQUIRED and `ask_owner`.
#[test]
fn an_answered_decision_in_a_new_revision_is_not_asked_again() {
    let (_p, s, task) = started();
    let fail = |revision: i64, node: &str, detail: &str| {
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=?,plan_json='{\"tasks\":[],\"verification\":[]}' WHERE task_id=?",
                (revision, &task))?;
            let failed = json!({"assignment_outcome":"failed","report_ref":"r_aaaaaaaaaaaa","checks":[],"summary":"stuck",
                "blocker":{"kind":"owner_decision","detail":detail}});
            db.execute("INSERT INTO runtime_nodes VALUES (?,?,?,'worker-backend-dev','A','[]','[\"src\"]','execution','failed',?,?)",
                (&task, revision, node, format!("asg-{node}"), failed.to_string()))?;
            obstacles::classify_failed_tx(db, &task)?;
            Ok(())
        })
        .unwrap()
    };
    fail(1, "export", "Keep the deprecated /v1/export endpoint for existing clients or remove it in this release?");
    assert_eq!(count(&s, &task, "OWNER_DECISION_REQUIRED"), 1);
    s.write(|db| {
        answered(
            db,
            &task,
            "pq_d8e38b669c30480da5cd3adbfa0cce45",
            "Keep /v1/export?",
            "Remove it in this release",
        );
        Ok(())
    })
    .unwrap();
    // The next plan's worker words the same decision differently.
    fail(2, "export_cleanup", "Whether to keep the deprecated /v1/export endpoint for existing clients or remove it is the owner's choice.");
    assert_eq!(count(&s, &task, "OWNER_DECISION_REQUIRED"), 1);
    let classified = latest(&s, &task, "FAILURE_CLASSIFIED");
    assert_eq!(
        (classified["class"].as_str(), classified["rule"].as_str()),
        (Some("technical"), Some("owner_decision_answered"))
    );
    assert!(classified["detail"]
        .as_str()
        .unwrap()
        .contains("Remove it in this release"));
    let checks = node_result(&s, &task, "asg-export_cleanup")["checks"].clone();
    assert_eq!(checks[0]["outcome"], "unverified");
    assert_eq!(checks[0]["source"], "runtime");
    assert_eq!(latest(&s, &task, "LOOP_BROKEN")["kind"], "owner_decision");
    // Not parked: the ordinary technical route applies, no owner stop.
    s.write(|db| {
        assert!(obstacles::unparked_failure(db, &task)?);
        Ok(())
    })
    .unwrap();

    // The review route: every owner item of the stop was decided, so the stop is no
    // `ask_owner` continuation either.
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLANNING',revision=3,blocker=NULL WHERE task_id=?", [&task])?;
        lifecycle::event_tx(db, &task, "REVIEW_DEAD_END_ROUTED", json!({"review":"_review_1","revision":3,"verdict":"INCOMPLETE",
            "blocked":[{"blocker":"owner_decision","item":"src/api/v1/export.rs",
                "reason":"Owner answers pq_d8e38b669c30480da5cd3adbfa0cce45 decided removal"},
                {"blocker":"other_role_work","item":"docs/api.md","reason":"the writer must update it"}]}))?;
        domain::stop_tx(db, &task, "review_incomplete")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, &task, "OWNER_DECISION_REQUIRED"), 1);
    let cont = latest(&s, &task, "AUTOMATIC_CONTINUATION");
    assert_ne!(cont["strategy"], "ask_owner");
    assert_eq!(cont["class"], "technical");
}

/// A dismissed (or resolved but still failing) access action is opened at most
/// twice; the third failure is technical with the missing access disclosed.
#[test]
fn a_dismissed_access_action_is_not_reopened_a_third_time() {
    let (_p, s, task) = started();
    for revision in 1..=3 {
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=?,plan_json='{\"tasks\":[],\"verification\":[]}' WHERE task_id=?",
                (revision, &task))?;
            let failed = json!({"assignment_outcome":"failed","report_ref":"r_aaaaaaaaaaaa","checks":[],"summary":"cannot open the PR",
                "blocker":{"kind":"missing_access","detail":"gh says: not logged into any GitHub hosts. Run gh auth login"}});
            db.execute("INSERT INTO runtime_nodes VALUES (?,?,'publish','worker-devops-engineer','PR','[]','[\".\"]','execution','failed',?,?)",
                (&task, revision, format!("asg-{revision}"), failed.to_string()))?;
            obstacles::classify_failed_tx(db, &task)?;
            if revision < 3 {
                let action = obstacles::open_actions(db, &task)?.pop().expect("an open action");
                lifecycle::event_tx(db, &task, "OWNER_ACTION_RESOLVED", json!({"id":action["id"],"how":"dismissed"}))?;
            }
            Ok(())
        })
        .unwrap();
    }
    assert_eq!(count(&s, &task, "OWNER_ACTION_REQUIRED"), 2);
    let classified = latest(&s, &task, "FAILURE_CLASSIFIED");
    assert_eq!(
        (classified["class"].as_str(), classified["rule"].as_str()),
        (Some("technical"), Some("access_repeats"))
    );
    assert_eq!(classified["obstacle"], Value::Null);
    let checks = node_result(&s, &task, "asg-3")["checks"].clone();
    assert_eq!(checks[0]["name"], "Access to github not available");
    assert_eq!(checks[0]["outcome"], "unverified");
    assert_eq!(latest(&s, &task, "LOOP_BROKEN")["kind"], "missing_access");
}

/// AI-Frontier t_a1a28b962190 324515: a registry mirror's 429 is an outage wait, not
/// missing access, and the runtime ends that wait by time alone and retries.
#[test]
fn a_rate_limit_from_any_host_waits_instead_of_asking_for_access() {
    let blocker = json!({"kind":"missing_access","detail":"The dev Compose stack is not running: no containers exist and the afi-web-dev image is missing. Inside the guarded sandbox, `docker compose -f compose.dev.yml up -d` exited 1. The image pull returned 429 from mirror.gcr.io, and the local build fallback then failed with 'open /home/u/.docker/buildx/activity/x: read-only file system'. The owner needs to start the stack with `docker compose -f compose.dev.yml up -d`."});
    let found = obstacles::classify(&obstacles::Signals {
        blocker_kind: Some("missing_access".into()),
        blocker_detail: blocker["detail"].as_str().map(str::to_owned),
        ..Default::default()
    });
    assert_eq!(
        (found.class, found.rule),
        (obstacles::EXTERNAL_OUTAGE, "rate_limited_timer")
    );
    assert_eq!(
        (found.host.as_deref(), found.probe),
        (Some("mirror.gcr.io"), Some(obstacles::TIMER))
    );
    // A full disk is not a rate limit.
    let disk = obstacles::classify(&obstacles::Signals {
        blocker_kind: Some("tool_unavailable".into()),
        blocker_detail: Some("npm ci failed: Disk quota exceeded".into()),
        ..Default::default()
    });
    assert_eq!(disk.class, obstacles::TECHNICAL);

    let (_p, s, task) = started();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=3,plan_json='{\"tasks\":[],\"verification\":[]}' WHERE task_id=?", [&task])?;
        let failed = json!({"assignment_outcome":"failed","report_ref":"r_55dd1bc600a7","checks":[],"summary":"typecheck not run","blocker":blocker});
        db.execute("INSERT INTO runtime_nodes VALUES (?,3,'Typecheck_repair','worker-frontend-dev','T','[]','[\"apps/web\"]','execution','failed','asg-t',?)",
            (&task, failed.to_string()))?;
        obstacles::classify_failed_tx(db, &task)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, &task, "OWNER_ACTION_REQUIRED"), 0);
    let wait = latest(&s, &task, "EXTERNAL_WAIT_STARTED");
    assert_eq!(
        (wait["host"].as_str(), wait["probe"].as_str()),
        (Some("mirror.gcr.io"), Some("timer"))
    );
    let id = wait["id"].as_str().unwrap().to_owned();
    let due = wait["next_check_at"].as_i64().unwrap();
    // The host is never probed: the runtime's failed (unsupported) probe still ends
    // the wait once its backoff elapsed, and the parked node is retried.
    assert!(s
        .write(|db| obstacles::record_recheck_tx(
            db,
            &task,
            &id,
            false,
            due,
            "outage probe destination unsupported"
        ))
        .unwrap());
    assert_eq!(
        latest(&s, &task, "EXTERNAL_WAIT_ENDED")["reason"],
        "backoff_elapsed"
    );
    assert_eq!(
        s.write(|db| obstacles::retry_unblocked_tx(db, &task))
            .unwrap(),
        vec!["Typecheck_repair".to_owned()]
    );
}

/// t_e8b28e435551 16136/16202: the same question with the same options is answered
/// from the record; the owner is not asked twice. Coordinator and worker routes.
#[test]
fn a_duplicate_question_returns_the_earlier_answer() {
    let p = Project::new();
    let s = p.open();
    let owner = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    let question = "Как поступить с планом спринта первой серии «Life Is Broken»?";
    let options = json!([
        "Исправить команду проверки перевода в t7 и отправить план на ещё одну проверку",
        "Завершить без нового плана спринта"
    ]);
    let asked = s
        .write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET state='PLANNING',revision=0 WHERE task_id='t'",
                [],
            )?;
            skill_proposals::request(
                db,
                &p.0,
                "t",
                "coord",
                "q1",
                &json!({"question":question,"options":options}),
            )
        })
        .unwrap();
    assert_eq!(asked["state"], "WAITING_USER");
    let id = asked["question_id"].as_str().unwrap().to_owned();
    s.write(|db| {
        assert!(waits::answer_tx(
            db,
            "t",
            &id,
            "Исправить команду проверки перевода в t7 и отправить план на ещё одну проверку",
            "text",
            None
        )?);
        assert_eq!(domain::task_tx(db, "t")?.0, TaskState::Planning);
        Ok(())
    })
    .unwrap();
    // Asked again with other spacing, case and option order.
    let again = s
        .write(|db| {
            skill_proposals::request(
                db,
                &p.0,
                "t",
                "coord",
                "q2",
                &json!({
                "question":"как поступить с  планом спринта первой серии «Life Is Broken»",
                "options":[options[1], options[0]]}),
            )
        })
        .unwrap();
    assert_eq!(again["outcome"], "refused");
    assert_eq!(again["error"], "question_already_answered");
    assert_eq!(again["question_id"], json!(id));
    assert_eq!(
        again["answer"],
        "Исправить команду проверки перевода в t7 и отправить план на ещё одну проверку"
    );
    s.read(|db| {
        let open: i64 = db.query_row(
            "SELECT count(*) FROM pending_questions WHERE task_id='t'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(open, 1);
        let state: String = db.query_row(
            "SELECT state FROM runtime_tasks WHERE task_id='t'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(state, "PLANNING");
        Ok(())
    })
    .unwrap();
    assert_eq!(latest(&s, "t", "LOOP_BROKEN")["kind"], "owner_question");

    // A worker asking the same question gets the same answer and opens no wait.
    let reply = s
        .write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'work','worker-backend-dev','Work','[]','[\"src\"]','execution','running','assignment',NULL)", [])?;
            let c = cortex_core::channels::issue_tx(db, "t", &owner, cortex_core::channels::Principal::Worker { assignment: "assignment" })?;
            cortex_core::channels::bind_session_tx(db, &c, "engine")?;
            waits::request_tx_bound(db, "t", &c.channel_id, "wq", &[waits::QuestionForm {
                question: question.into(),
                options: options.as_array().unwrap().iter().map(|o| o.as_str().unwrap().to_owned()).collect(),
            }])
        })
        .unwrap();
    assert_eq!(reply["error"], "question_already_answered");
    assert_eq!(reply["status"], "answered");
    assert_eq!(count(&s, "t", "WORKER_WAIT_CHANGED"), 0);

    // An owner message opens a new window: the question may be asked again.
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'",
            [],
        )?;
        lifecycle::event_tx(
            db,
            "t",
            "USER_STEERING",
            json!({"text":"I changed my mind about t7"}),
        )?;
        let asked = skill_proposals::request(
            db,
            &p.0,
            "t",
            "coord",
            "q3",
            &json!({"question":question,"options":options}),
        )?;
        assert_eq!(asked["state"], "WAITING_USER");
        Ok(())
    })
    .unwrap();
}

fn findings(items: &[(&str, &str)]) -> Value {
    json!(items
        .iter()
        .map(|(file, fix)| json!({"node":"ru-deploy-guide","files":[file],"cross_cutting":false,"fix":fix}))
        .collect::<Vec<_>>())
}

/// One plan revision with an implementing node and its completed review `n`.
fn review_round(s: &Store, task: &str, revision: i64, n: i64, digest: &str, found: &Value) {
    s.write(|db| {
        let plan = json!({"tasks":[{"id":"ru-deploy-guide","role":"worker-technical-writer","objective":"Guide",
            "dependencies":[],"scope":["docs"]}],"verification":[["make","docs"]]});
        db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=?,review_cycle=?,plan_json=? WHERE task_id=?",
            (revision, n, plan.to_string(), task))?;
        db.execute("INSERT OR IGNORE INTO runtime_nodes VALUES (?,?,'ru-deploy-guide','worker-technical-writer','Guide','[]','[\"docs\"]','execution','completed',?,'{}')",
            (task, revision, format!("w{revision}")))?;
        let result = json!({"verdict":"REQUEST_CHANGES","report_ref":format!("r{revision}{n}"),
            "project_digest":digest.repeat(64),"findings":found});
        db.execute("INSERT INTO runtime_nodes VALUES (?,?,?,'worker-code-reviewer','Review','[]','[\"**\"]','review','completed',?,?)",
            (task, revision, format!("_review_{n}"), format!("rv{revision}{n}"), result.to_string()))?;
        review::advance_tx(db, task)
    })
    .unwrap();
}

fn fixes(s: &Store, task: &str, revision: i64) -> i64 {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT count(*) FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id LIKE '\\_fix\\_%' ESCAPE '\\'",
            (task, revision),
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

/// The same findings in a new plan revision stop the fix cycle (no fresh budget per
/// revision): `REVIEW_NO_PROGRESS` across revisions and `LOOP_OBSERVED`.
#[test]
fn identical_findings_across_revisions_break_instead_of_cycling() {
    let (_p, s, task) = started();
    let same = findings(&[(
        "docs/ru/deployment.md",
        "Keep ingress and writers isolated while validating restoration.",
    )]);
    review_round(&s, &task, 1, 1, "a", &same);
    assert_eq!(fixes(&s, &task, 1), 1);
    // A continuation planned a new revision; its first review repeats the findings
    // (only spacing and case differ).
    let reworded = findings(&[(
        "docs/ru/deployment.md",
        "keep ingress  and writers isolated while validating restoration.",
    )]);
    review_round(&s, &task, 2, 1, "b", &reworded);
    assert_eq!(fixes(&s, &task, 2), 0);
    let stop = latest(&s, &task, "REVIEW_NO_PROGRESS");
    assert_eq!(stop["reason"], "repeated_findings");
    assert_eq!(
        (
            stop["across_revisions"].as_bool(),
            stop["previous_revision"].as_i64()
        ),
        (Some(true), Some(1))
    );
    let observed = observed(&s, &task, "review_findings");
    assert_eq!(
        (observed["kind"].as_str(), observed["count"].as_i64()),
        (Some("review_findings"), Some(2))
    );
    assert_eq!(observed["key"], stop["key"]);
    assert_eq!(
        latest(&s, &task, "AUTOMATIC_CONTINUATION")["reason"],
        "review_no_progress"
    );
}

/// Business t_d530de978253: after three change-requesting reviews in the lineage, a
/// new revision gets further fix rounds only while the findings strictly shrink.
#[test]
fn shrinking_findings_still_allow_rounds_and_growing_ones_stop() {
    let (_p, s, task) = started();
    let f = |n: usize, tag: &str| {
        findings(
            &(0..n)
                .map(|i| {
                    (
                        "docs/ru/deployment.md",
                        Box::leak(
                            format!("{tag} step {i}: gate it on its exit status").into_boxed_str(),
                        ) as &str,
                    )
                })
                .collect::<Vec<_>>(),
        )
    };
    review_round(&s, &task, 1, 1, "a", &f(5, "bootstrap"));
    review_round(&s, &task, 1, 2, "b", &f(2, "restore"));
    review_round(&s, &task, 1, 3, "c", &f(6, "rollback"));
    assert_eq!(fixes(&s, &task, 1), 2);
    assert_eq!(
        latest(&s, &task, "AUTOMATIC_CONTINUATION")["reason"],
        "review_cycle_limit"
    );
    // Revision 2: fewer findings than the last review: another fix round.
    review_round(&s, &task, 2, 1, "d", &f(4, "count"));
    assert_eq!(fixes(&s, &task, 2), 1);
    assert_eq!(
        latest(&s, &task, "REVIEW_CHANGES_REQUESTED")["review"],
        "_review_1"
    );
    // Still shrinking: one more.
    review_round(&s, &task, 2, 2, "e", &f(2, "schema"));
    assert_eq!(fixes(&s, &task, 2), 2);
    // Growing again: the lineage budget refuses the round, without a fresh one.
    let continuations = count(&s, &task, "AUTOMATIC_CONTINUATION");
    review_round(&s, &task, 3, 1, "f", &f(3, "checkout"));
    assert_eq!(fixes(&s, &task, 3), 0);
    assert_eq!(
        count(&s, &task, "AUTOMATIC_CONTINUATION"),
        continuations + 1
    );
    assert_eq!(
        latest(&s, &task, "AUTOMATIC_CONTINUATION")["reason"],
        "review_cycle_limit"
    );
    let observed = observed(&s, &task, "review_cycle");
    assert_eq!(
        (
            observed["kind"].as_str(),
            observed["findings"].as_i64(),
            observed["previous_findings"].as_i64()
        ),
        (Some("review_cycle"), Some(3), Some(2))
    );
}
