//! Failure classification and its routing (access, owner decision, outage, technical).
mod support;
use cortex_core::{
    lifecycle::{self, automatic, domain, obstacles},
    workspace::{self, NativeExecutionPolicy},
    Store, TaskState,
};
use serde_json::{json, Value};
use support::Project;

fn signals(kind: Option<&str>, detail: &str, argv: &[&str], output: &str) -> obstacles::Signals {
    obstacles::Signals {
        blocker_kind: kind.map(str::to_owned),
        blocker_detail: (!detail.is_empty()).then(|| detail.to_owned()),
        texts: vec![],
        outputs: if argv.is_empty() {
            vec![]
        } else {
            vec![obstacles::Output {
                argv: argv.iter().map(|a| a.to_string()).collect(),
                text: output.into(),
            }]
        },
        attempted: None,
    }
}

/// A worker that ran nothing and met no refusal cannot park its work on "no
/// access": the claim is a technical failure the next attempt corrects by trying
/// (owner store 2026-11-23). A quoted login error or a real refusal still counts.
#[test]
fn an_untried_access_claim_is_technical_but_a_real_refusal_is_access() {
    let claim = "No authorized execution-and-writeback route to the GPU host is available to this worker";
    let mut s = signals(Some("missing_access"), claim, &[], "");
    s.attempted = Some(false);
    let found = obstacles::classify(&s);
    assert_eq!((found.class, found.rule), ("technical", "blocker_missing_access_unattempted"), "{found:?}");
    assert!(found.detail.contains("nothing was tried"), "{found:?}");
    s.attempted = Some(true);
    assert_eq!(obstacles::classify(&s).class, "access");
    s.attempted = None;
    assert_eq!(obstacles::classify(&s).class, "access");
    let mut quoted = signals(Some("missing_access"), "gh says: not logged into any GitHub hosts", &[], "");
    quoted.attempted = Some(false);
    assert_eq!(obstacles::classify(&quoted).class, "access");
}

#[test]
fn classification_table_from_realistic_receipts_and_blockers() {
    let cases: Vec<(obstacles::Signals, &str, Option<&str>)> = vec![
        (signals(Some("tool_unavailable"), "cannot open the PR", &["gh", "pr", "create"],
            "To get started with GitHub CLI, please run:  gh auth login\n"), "access", Some("github")),
        (signals(None, "", &["curl", "-fsS", "https://api.openai.com/v1/models"],
            "curl: (22) The requested URL returned error: 401"), "access", Some("codex")),
        (signals(Some("missing_access"), "the deploy needs administrator rights", &["sudo", "apt-get", "install", "x"],
            "sudo: a password is required"), "access", Some("sudo")),
        (signals(Some("owner_decision"), "keep the v1 API or break clients?", &[], ""), "owner_decision", None),
        (signals(Some("tool_unavailable"), "The spec requires offline mode but also live sync; these requirements conflict", &[], ""),
            "owner_decision", None),
        (signals(None, "", &["curl", "https://registry.example.org/pkg"],
            "curl: (6) Could not resolve host: registry.example.org"), "technical", None),
        (signals(None, "", &["git", "fetch"],
            "fatal: unable to access 'https://git.example.com/repo.git/': The requested URL returned error: 503"), "technical", None),
        (signals(None, "", &["gh", "api", "repos/x/y"],
            "HTTP 429: API rate limit exceeded (https://api.github.com/repos/x/y)"), "external_outage", None),
        (signals(None, "", &["cargo", "test"],
            "thread 'db' panicked: connection refused (http://127.0.0.1:5432)"), "technical", None),
        (signals(Some("other_role_work"), "needs the backend fix first", &[], ""), "technical", None),
        (signals(None, "", &["cargo", "test"], "test result: FAILED. 3 passed; 1 failed"), "technical", None),
        (signals(None, "", &["cargo", "test"], "assertion failed: expected status 401"), "technical", None),
        (signals(None, "", &["/home/worker/.cargo/bin/cargo", "test"], "test auth::unauthorized ... FAILED"), "technical", None),
        (signals(None, "", &["npm", "test"], "expected Unauthorized but got OK"), "technical", None),
        (signals(None, "", &["npm", "run", "test:api"], "expected status 401"), "technical", None),
        (signals(None, "", &["cargo", "publish"], "error: registry returned status 403"), "access", Some("cargo")),
        (signals(Some("missing_access"), "Access to https://registry.npmjs.org/ is needed", &[], ""),
            "access", Some("registry.npmjs.org")),
    ];
    for (s, class, service) in cases {
        let found = obstacles::classify(&s);
        assert_eq!(found.class, class, "{s:?} -> {found:?}");
        if let Some(service) = service {
            assert_eq!(found.service.as_deref(), Some(service), "{found:?}");
        }
        if class == "external_outage" {
            assert!(found.host.is_some() && found.probe.is_some(), "{found:?}");
        }
    }
    assert_eq!(obstacles::backoff(0), 60);
    assert_eq!(obstacles::backoff(3), 600);
    assert_eq!(obstacles::backoff(99), 1800);
}

#[test]
fn outage_evidence_cannot_authorize_a_new_or_ambiguous_destination() {
    for destination in [
        "https://untrusted.example/",
        "https://api.github.com.untrusted.example/",
        "https://api.github.com:8443/",
        "https://api.github.com:65979/",
        "https://api.github.com:invalid/",
        "http://api.github.com/",
        "https://api.github.com/ or https://untrusted.example/",
        "https://api.github.com/ or https://api.openai.com/",
        "https://api.github.com/ or https://api.github.com:8443/",
    ] {
        let result = obstacles::classify(&signals(
            Some("tool_unavailable"),
            &format!("HTTP 503 from {destination}"),
            &[],
            "",
        ));
        assert_eq!(result.class, obstacles::TECHNICAL, "{destination}");
        assert_eq!(result.rule, "outage_probe_unsupported", "{destination}");
        assert_eq!(result.host, None, "{destination}");
        assert_eq!(result.probe, None, "{destination}");
    }
}

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

/// Revision 1 executing: `a` failed with `blocker`, `b` independent, `c` depends on `a`.
fn plan(s: &Store, task: &str, blocker: Value) {
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1,plan_json='{\"tasks\":[],\"verification\":[]}' WHERE task_id=?", [task])?;
        let failed = json!({"assignment_outcome":"failed","report_ref":"r_aaaaaaaaaaaa","checks":[],"summary":"stuck","blocker":blocker});
        db.execute("INSERT INTO runtime_nodes VALUES (?,1,'a','backend-dev','A','[]','[\"src\"]','execution','failed','asg-a',?)", (task, failed.to_string()))?;
        db.execute("INSERT INTO runtime_nodes VALUES (?,1,'b','backend-dev','B','[]','[\"lib\"]','execution','pending',NULL,NULL)", [task])?;
        db.execute("INSERT INTO runtime_nodes VALUES (?,1,'c','backend-dev','C','[\"a\"]','[\"src\"]','execution','pending',NULL,NULL)", [task])?;
        Ok(())
    })
    .unwrap();
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
        let raw: String = db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type=? ORDER BY id DESC LIMIT 1", (task, kind), |r| r.get(0))?;
        Ok(serde_json::from_str(&raw)?)
    })
    .unwrap()
}

#[test]
fn access_parks_its_node_once_unrelated_work_continues_and_resolution_retries_it() {
    let (_p, s, task) = started();
    plan(
        &s,
        &task,
        json!({"kind":"missing_access","detail":"gh says: not logged into any GitHub hosts. Run gh auth login"}),
    );
    s.write(|db| {
        obstacles::classify_failed_tx(db, &task)?;
        obstacles::classify_failed_tx(db, &task)?;
        // `b` is unrelated and still ready: nothing pauses, nothing stops.
        assert!(!obstacles::unparked_failure(db, &task)?);
        assert!(!obstacles::settle_parked_tx(db, &task)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, &task, "OWNER_ACTION_REQUIRED"), 1);
    let action = latest(&s, &task, "OWNER_ACTION_REQUIRED");
    assert_eq!(action["kind"], "access");
    assert_eq!(action["service"], "github");
    assert_eq!(action["node"], "a");
    assert_eq!(action["revision"], 1);
    assert!(action["id"].as_str().unwrap().starts_with("oa_"));
    // `b` finishes; only `c` (behind `a`) remains: the plan pauses without a rung.
    s.write(|db| {
        db.execute("UPDATE runtime_nodes SET status='completed',result_json='{}' WHERE task_id=? AND node_id='b'", [&task])?;
        assert!(obstacles::settle_parked_tx(db, &task)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
    let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
    assert_eq!(owed.waiting, vec![action["id"].clone()]);
    assert_eq!(
        s.write(|db| automatic::resume_tx(db, &task, owed.event))
            .unwrap(),
        None
    );
    // The owner resolves it: the same plan continues and `a` is retried at once.
    s.write(|db| {
        lifecycle::event_tx(
            db,
            &task,
            "OWNER_ACTION_RESOLVED",
            json!({"id":action["id"]}),
        )?;
        Ok(())
    })
    .unwrap();
    let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
    assert!(owed.waiting.is_empty());
    let resumed = s
        .write(|db| automatic::resume_tx(db, &task, owed.event))
        .unwrap()
        .unwrap();
    assert_eq!(resumed["resume"], "execution");
    let (state, status) = s
        .write(|db| {
            let state = domain::task_tx(db, &task)?.0;
            let status: String = db.query_row(
                "SELECT status FROM runtime_nodes WHERE task_id=? AND node_id='a'",
                [&task],
                |r| r.get(0),
            )?;
            Ok((state, status))
        })
        .unwrap();
    assert_eq!((state, status.as_str()), (TaskState::Executing, "pending"));
    assert_eq!(
        latest(&s, &task, "ASSIGNMENT_RETRIED")["obstacle_retry"],
        true
    );
}

#[test]
fn an_owner_decision_is_answerable_while_an_independent_node_runs() {
    let (project, s, task) = started();
    plan(&s, &task, json!({"kind":"owner_decision","detail":"Keep the v1 endpoint or remove it?"}));
    let question = s.write(|db| {
        db.execute("UPDATE runtime_nodes SET status='running',assignment_ref='asg-b' WHERE task_id=? AND node_id='b'", [&task])?;
        obstacles::classify_failed_tx(db, &task)?;
        obstacles::classify_failed_tx(db, &task)?;
        assert!(!obstacles::settle_parked_tx(db, &task)?);
        assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Executing);
        Ok(db.query_row("SELECT id FROM pending_questions WHERE task_id=? AND assignment_ref='asg-a' AND status='open'",
            [&task], |r| r.get::<_, String>(0))?)
    }).unwrap();
    assert_eq!(count(&s, &task, "OWNER_DECISION_REQUIRED"), 1);
    assert_eq!(count(&s, &task, "USER_QUESTION_REQUESTED"), 1);
    assert_eq!(count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
    drop(s);
    let s = project.open();
    let decision = workspace::control::decisions(&s, "p", "owner",
        &json!({"operation":"decisions.get","target":format!("wd_{question}")})).unwrap();
    assert_eq!(decision["status"], "pending");
    assert_eq!(decision["scope"], "asg-a");
    assert_eq!(decision["allowedOperations"], json!(["decisions.answer"]));
    assert_eq!(decision["options"], json!([]));
    let answer = json!({
        "protocol":{"major":1,"minor":8}, "requestId":"answer-v1", "projectId":"p", "actorId":"owner",
        "operation":"decisions.answer", "target":decision["decisionId"], "expectedVersion":decision["version"],
        "ownerEpoch":decision["ownerEpoch"],
        "payload":{"executionId":decision["executionId"], "acpSessionId":decision["acpSessionId"], "batchId":null,
            "answers":[{"decisionId":decision["decisionId"],"questionId":question,
                "expectedVersion":decision["version"], "optionId":null, "text":"Keep v1"}]}
    });
    workspace::control::answer(&s, "p", "owner", &answer).unwrap();
    s.write(|db| {
        assert!(!lifecycle::waits::answer_tx(db, &task, &question, "Remove v1", "text", None)?);
        assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Executing);
        assert!(obstacles::open_decisions(db, &task)?.is_empty());
        assert_eq!(obstacles::retry_decision_answer(db, &task, "a", 1)?.as_deref(), Some("Keep v1"));
        assert_eq!(obstacles::retry_decision_answer(db, &task, "b", 1)?, None);
        let sibling: String = db.query_row("SELECT status FROM runtime_nodes WHERE task_id=? AND node_id='b' AND assignment_ref='asg-b'",
            [&task], |r| r.get(0))?;
        assert_eq!(sibling, "running");
        Ok(())
    }).unwrap();
    assert_eq!(count(&s, &task, "ASSIGNMENT_RETRIED"), 1);
    assert_eq!(latest(&s, &task, "ASSIGNMENT_RETRIED")["previous_assignment"], "asg-a");
}

#[test]
fn environment_deadline_survives_store_reopen_and_terminal_cancel_closes_card() {
    let (project, s, task) = started();
    plan(&s, &task, json!({"kind":"missing_access","detail":"gh says: not logged into any GitHub hosts"}));
    let id = s.write(|db| {
        obstacles::classify_failed_tx(db, &task)?;
        let ids = obstacles::claim_environment_rechecks_tx(db, &task, 100)?;
        assert_eq!(ids.len(), 1);
        assert!(obstacles::claim_environment_rechecks_tx(db, &task, 100)?.is_empty());
        Ok(ids[0].clone())
    }).unwrap();
    drop(s);
    let s = project.open();
    s.write(|db| {
        assert!(obstacles::claim_environment_rechecks_tx(db, &task, 159)?.is_empty());
        assert_eq!(obstacles::claim_environment_rechecks_tx(db, &task, 160)?, vec![id.clone()]);
        assert!(obstacles::claim_environment_rechecks_tx(db, &task, 279)?.is_empty());
        domain::cancel_task_tx(db, &task, "owner_cancelled")?;
        assert!(obstacles::open_actions(db, &task)?.is_empty());
        assert!(obstacles::claim_environment_rechecks_tx(db, &task, 280)?.is_empty());
        assert!(obstacles::resolve_environment_actions_tx(db, &task,
            &json!({"gh_auth":{"status":"logged_in"}}), Some(&[id.clone()]))?.is_empty());
        Ok(())
    }).unwrap();
    assert_eq!(count(&s, &task, "OWNER_ACTION_RESOLVED"), 1);
    assert_eq!(latest(&s, &task, "OWNER_ACTION_RESOLVED")["by"], "task_terminal");
}


#[test]
fn genuine_completion_closes_remaining_owner_action_without_granting_access() {
    let p = Project::new();
    let s = p.open();
    lifecycle::TaskMachine::new(&s).create("answer", "One answer", "owner").unwrap();
    s.write(|db| {
        let found = obstacles::classify(&signals(Some("missing_access"), "GitHub is not logged in", &[], ""));
        let id = obstacles::open_obstacle_tx(db, "answer", &found, None, 0)?.unwrap();
        db.execute("UPDATE runtime_tasks SET state='ANSWER_READY',synthesis='Retained answer' WHERE task_id='answer'", [])?;
        lifecycle::event_tx(db, "answer", "ANSWER_SUBMITTED", json!({"text":"Retained answer"}))?;
        lifecycle::completion::complete_tx(db, "answer", &Default::default())?;
        assert_eq!(domain::task_tx(db, "answer")?.0, TaskState::Completed);
        assert!(obstacles::open_actions(db, "answer")?.is_empty());
        assert!(!obstacles::open_actions(db, "answer")?.iter().any(|a| a["id"] == id));
        Ok(())
    }).unwrap();
    assert_eq!(count(&s, "answer", "OWNER_ACTION_RESOLVED"), 1);
    assert_eq!(latest(&s, "answer", "OWNER_ACTION_RESOLVED")["state"], "COMPLETED");
}

#[test]
fn an_unsupported_outage_does_not_start_a_wait() {
    let (_p, s, task) = started();
    plan(
        &s,
        &task,
        json!({"kind":"tool_unavailable","detail":"Failed to connect to untrusted.example port 443"}),
    );
    s.write(|db| {
        obstacles::classify_failed_tx(db, &task)?;
        assert!(obstacles::unparked_failure(db, &task)?);
        assert!(obstacles::due_waits(db, &task, i64::MAX)?.is_empty());
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, &task, "EXTERNAL_WAIT_STARTED"), 0);
}

#[test]
fn an_outage_waits_with_backoff_and_a_successful_recheck_retries() {
    let (_p, s, task) = started();
    plan(
        &s,
        &task,
        json!({"kind":"tool_unavailable","detail":"curl: (7) Failed to connect to api.github.com port 443: Connection refused"}),
    );
    s.write(|db| {
        db.execute("UPDATE runtime_nodes SET status='completed',result_json='{}' WHERE task_id=? AND node_id='b'", [&task])?;
        obstacles::classify_failed_tx(db, &task)?;
        assert!(obstacles::settle_parked_tx(db, &task)?);
        Ok(())
    })
    .unwrap();
    let wait = latest(&s, &task, "EXTERNAL_WAIT_STARTED");
    assert_eq!(
        (
            wait["host"].as_str(),
            wait["port"].as_i64(),
            wait["probe"].as_str()
        ),
        (Some("api.github.com"), Some(443), Some("tcp"))
    );
    let id = wait["id"].as_str().unwrap().to_owned();
    let now = wait["next_check_at"].as_i64().unwrap();
    assert!(s
        .read(|db| obstacles::due_waits(db, &task, now - 1))
        .unwrap()
        .is_empty());
    assert!(!s
        .write(|db| obstacles::record_recheck_tx(db, &task, &id, false, now, "refused"))
        .unwrap());
    assert_eq!(
        latest(&s, &task, "EXTERNAL_WAIT_RECHECKED")["next_check_at"],
        now + 120
    );
    assert!(s
        .write(|db| obstacles::record_recheck_tx(db, &task, &id, true, now + 120, "ok"))
        .unwrap());
    assert_eq!(
        latest(&s, &task, "EXTERNAL_WAIT_ENDED")["reason"],
        "recheck_succeeded"
    );
    assert_eq!(count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
    let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
    let resumed = s
        .write(|db| automatic::resume_tx(db, &task, owed.event))
        .unwrap()
        .unwrap();
    assert_eq!(resumed["retried"], json!(["a"]));
}

#[test]
fn a_network_command_does_not_make_another_receipts_test_output_access_evidence() {
    let mut s = signals(
        None,
        "",
        &["curl", "https://registry.npmjs.org/"],
        "HTTP 404",
    );
    let test = signals(
        None,
        "",
        &["cargo", "test"],
        "expected status 401 Unauthorized",
    );
    s.outputs.splice(0..0, test.outputs);
    s.texts.push("test failed: unauthorized".into());
    assert_eq!(obstacles::classify(&s).class, obstacles::TECHNICAL);

    s.outputs[1].text = "HTTP 401 Unauthorized".into();
    let found = obstacles::classify(&s);
    assert_eq!(found.class, obstacles::ACCESS);
    assert_eq!(found.service.as_deref(), Some("registry.npmjs.org"));
}

#[test]
fn owner_action_service_keys_accept_registry_hosts_but_not_urls() {
    for valid in [
        "sudo",
        "private_registry",
        "registry.npmjs.org",
        "packages.long-company-subdomain.example-registry.org",
    ] {
        assert!(obstacles::access_service_ok(valid), "{valid}");
    }
    for invalid in [
        "",
        "https://registry.npmjs.org",
        "registry.npmjs.org:443",
        "registry.npmjs.org/login",
        "registry..org",
        "registry.-npmjs.org",
    ] {
        assert!(!obstacles::access_service_ok(invalid), "{invalid}");
    }
}

#[test]
fn non_approving_reviews_do_not_record_progress_even_with_passed_checks() {
    let (_p, s, task) = started();
    s.write(|db| {
        lifecycle::event_tx(db, &task, "PLAN_ACCEPTED", json!({"revision":1}))?;
        for verdict in ["REQUEST_CHANGES", "INCOMPLETE", "BLOCK"] {
            lifecycle::event_tx(
                db,
                &task,
                "WORKER_COMPLETED",
                json!({"result":{"verdict":verdict,"checks":[{"name":"review","outcome":"passed"}]}}),
            )?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, &task, "WORKER_COMPLETED"), 3);
    assert_eq!(count(&s, &task, "TASK_PROGRESS"), 1);
    assert_eq!(latest(&s, &task, "TASK_PROGRESS")["kind"], "plan_accepted");
}

#[test]
fn a_technical_failure_is_not_parked_and_progress_is_marked() {
    let (_p, s, task) = started();
    plan(
        &s,
        &task,
        json!({"kind":"tool_unavailable","detail":"the test suite fails with a type error"}),
    );
    s.write(|db| {
        obstacles::classify_failed_tx(db, &task)?;
        assert!(obstacles::unparked_failure(db, &task)?);
        assert!(!obstacles::settle_parked_tx(db, &task)?);
        lifecycle::event_tx(db, &task, "PLAN_ACCEPTED", json!({"revision":1}))?;
        db.execute(
            "UPDATE runtime_nodes SET assignment_ref='asg-b' WHERE task_id=? AND node_id='b'",
            [&task],
        )?;
        lifecycle::event_tx(
            db,
            &task,
            "WORKER_COMPLETED",
            json!({"assignment_ref":"asg-b",
            "result":{"checks":[{"name":"t","outcome":"passed"}]}}),
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        latest(&s, &task, "FAILURE_CLASSIFIED")["class"],
        "technical"
    );
    let kinds: Vec<String> = s.read(|db| {
        Ok(db.prepare("SELECT json_extract(payload_json,'$.kind') FROM runtime_events WHERE task_id=? AND type='TASK_PROGRESS' ORDER BY id")?
            .query_map([&task], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?)
    }).unwrap();
    assert_eq!(kinds, ["plan_accepted", "check_passed", "node_completed"]);
    assert_eq!(latest(&s, &task, "TASK_PROGRESS")["node"], "b");
}
