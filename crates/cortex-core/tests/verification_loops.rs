//! Verification, review coverage and completion gates never loop and never stop on
//! the environment: shapes copied from owner stores (2026-11-17..21).
mod support;
use cortex_core::{
    lifecycle::{self, completion::*, reconciliation::*, TaskMachine},
    ownership::OwnerEpoch,
    ports::CommandObservation,
    Store,
};
use serde_json::{json, Value};
use support::*;

fn setup() -> (Project, Store, OwnerEpoch, EvidenceSnapshot) {
    let p = Project::new();
    let s = p.open();
    let o = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    let e = EvidenceSnapshot {
        project_digest: Some("a".repeat(64)),
        ignored_digest: Some("1".repeat(64)),
        ..Default::default()
    };
    s.write(|db|{
        db.execute("UPDATE runtime_tasks SET state='PLAN_READY',revision=1,plan_json=? WHERE task_id='t'",
            [json!({"verification":[["cargo","test"]],"tasks":[]}).to_string()])?;
        execution_baseline_tx(db,"t",&e)?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'work','worker-backend-dev','Work','[]','[\"src\"]','execution','completed','a',?)",
            [json!({"report_ref":"r","report_digest":"f".repeat(64),"checks":[]}).to_string()])?;
        db.execute("INSERT INTO reports VALUES ('r','t','report','r.md','now')",[])?;
        add_review_tx(db,"t",&e)?;
        Ok(())
    }).unwrap();
    (p, s, o, e)
}
fn state(s: &Store) -> String {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT state FROM runtime_tasks WHERE task_id='t'",
            [],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}
fn events(s: &Store, kind: &str) -> Vec<Value> {
    s.read(|db| {
        let rows = db
            .prepare(
                "SELECT payload_json FROM runtime_events WHERE task_id='t' AND type=? ORDER BY id",
            )?
            .query_map([kind], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|r| serde_json::from_str(&r).unwrap())
            .collect())
    })
    .unwrap()
}
fn verify(s: &Store, o: &OwnerEpoch, e: &EvidenceSnapshot, key: &str, code: i32) {
    s.write(|db| {
        retain_check_tx(
            db,
            "t",
            o,
            key,
            &["cargo".into(), "test".into()],
            &CommandObservation {
                complete: true,
                exit_code: Some(code),
                receipt_ref: key.into(),
            },
            e,
        )?;
        record_verification_tx(db, "t", &[key.into()], e)?;
        Ok(())
    })
    .unwrap();
}
/// Back to VERIFYING after a failure, as an accepted repair or a resumed rung does.
fn reverify(s: &Store, resumed: bool) {
    s.write(|db| {
        if resumed {
            lifecycle::event_tx(db, "t", "AUTOMATIC_RESUMED", json!({}))?;
        }
        lifecycle::event_tx(
            db,
            "t",
            "VERIFICATION_REVISED",
            json!({"verification":[["cargo","test"]]}),
        )?;
        db.execute(
            "UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id='t'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
}

/// Verifier receipts whose outcome is the verifier's environment, not the work, from
/// real stores: output that never reached its end (cortex t_5715 293620), a bare
/// `cargo` the verifier did not find (cortex t_e308 274436), a disk quota (cortex
/// 284992), a process killed from outside before the deadline (Business 27036), and
/// a loopback service the verifier sandbox could not reach while the worker's
/// identical run passed on the same digest (Business 25327 against 25315).
#[test]
fn environmental_verifier_outcomes_are_classified_and_real_failures_stay_failures() {
    let unavailable = json!({"changed_paths":[],"complete":false,"exit_code":0,"failure":"command_output_unavailable",
        "output_truncated":true,"stderr":"","stdout":""});
    assert_eq!(
        verifier_environment(&unavailable, false),
        Some("output_unavailable")
    );
    let missing = json!({"complete":true,"exit_code":127,"failure":null,"stderr":"timeout: failed to run command 'cargo': No such file or directory\n","stdout":""});
    assert_eq!(
        verifier_environment(&missing, false),
        Some("program_not_found")
    );
    let denied = json!({"complete":true,"exit_code":126,"failure":null,"stderr":"","stdout":""});
    assert_eq!(verifier_environment(&denied, false), Some("not_executable"));
    let quota = json!({"complete":true,"exit_code":101,"failure":null,"stdout":"",
        "stderr":"error: failed to write /home/u/p/target/debug/deps/x.rmeta: Disk quota exceeded (os error 122)\n"});
    assert_eq!(verifier_environment(&quota, false), Some("disk_quota"));
    let full = json!({"complete":true,"exit_code":1,"failure":null,"stdout":"","stderr":"write error: No space left on device\n"});
    assert_eq!(verifier_environment(&full, false), Some("disk_full"));
    let killed = json!({"complete":false,"exit_code":null,"output_truncated":false,"stdout":"Running 246 tests using 1 worker\n","stderr":""});
    assert_eq!(verifier_environment(&killed, false), Some("incomplete"));
    let refused = json!({"complete":true,"exit_code":1,"output_truncated":false,"stderr":" Image bsd-test-db Built \n",
        "stdout":"not ok 1 - real Nest boundary\n  error: 'connect ECONNREFUSED 127.0.0.1:32770'\n  code: 'ECONNREFUSED'\n"});
    // Without a worker's identical pass on the same bytes this is a real failure.
    assert_eq!(verifier_environment(&refused, false), None);
    assert_eq!(
        verifier_environment(&refused, true),
        Some("network_refused")
    );
    let differs =
        json!({"complete":true,"exit_code":1,"failure":null,"stdout":"1 failed","stderr":""});
    assert_eq!(
        verifier_environment(&differs, true),
        Some("environment_difference")
    );
    assert_eq!(verifier_environment(&differs, false), None);
    // Real outcomes: a pass, a nonzero exit with complete output, output past the
    // retained limit, the verifier's own deadline, a signal and an owner stop.
    for real in [
        json!({"complete":true,"exit_code":0,"failure":null}),
        json!({"complete":true,"exit_code":1,"failure":null,"stdout":"assertion failed","stderr":""}),
        json!({"complete":false,"exit_code":0,"failure":"command_output_limit","output_truncated":true}),
        json!({"complete":false,"exit_code":null,"failure":"command_timeout"}),
        json!({"complete":false,"exit_code":null,"failure":"command_signalled"}),
        json!({"complete":false,"exit_code":null,"failure":"owner_stopped"}),
    ] {
        assert_eq!(verifier_environment(&real, false), None, "{real}");
    }
}

/// Business t_e2e854: the worker's `bash scripts/test-integration.sh` passed on
/// digest 0146c0… (25315) and the verifier's identical run failed on the same
/// digest (25327). Only the same argv on the same bytes counts.
#[test]
fn a_worker_pass_of_the_same_command_on_the_same_bytes_is_found() {
    let (_p, s, _o, _e) = setup();
    let digest = "0146c0241c133292c7a467555cb3f0f9b5b27424b4132c58c1b721cf4883c8b3";
    s.write(|db| {
        db.execute("INSERT INTO runtime_requests VALUES ('t','w1','ch_5adab0d0b820','command_run','x',?)",
            [json!({"access_mode":"auto","argv":["bash","scripts/test-integration.sh"],"byte_count":31758,"complete":true,
                "exit_code":0,"failure":null,"large_output_id":"lo_d78913fb56cb","output_truncated":false,"profile":"read_only",
                "revision_digest":digest,"settled":true,"signal":null,"tool_call_ref":"native-command:950448a4facea5e3884251fe"}).to_string()])?;
        db.execute("INSERT INTO runtime_requests VALUES ('t','w2','ch_5adab0d0b820','outside_command','y',?)",
            [json!({"reply":{"structuredContent":{"argv":["npm","test"],"complete":true,"exit_code":0,"failure":null,
                "revision_digest":digest}}}).to_string()])?;
        Ok(())
    })
    .unwrap();
    s.read(|db| {
        let argv = ["bash".to_owned(), "scripts/test-integration.sh".to_owned()];
        assert!(worker_passed_tx(db, "t", &argv, &[digest], &serde_json::Value::Null)?);
        assert!(worker_passed_tx(db, "t", &argv, &["", digest], &serde_json::Value::Null)?);
        assert!(!worker_passed_tx(db, "t", &argv, &[&"b".repeat(64)], &serde_json::Value::Null)?);
        assert!(!worker_passed_tx(db, "t", &["bash".to_owned()], &[digest], &serde_json::Value::Null)?);
        assert!(worker_passed_tx(
            db,
            "t",
            &["npm".to_owned(), "test".to_owned()],
            &[digest],
            &serde_json::Value::Null
        )?);
        Ok(())
    })
    .unwrap();
}

/// cortex t_5715 288531-288538: a restart during `scripts/cortex-xtask.sh test-fast`
/// left the run's intent without a finish record, every later verification of the
/// revision was refused `native_verification_outcome_unknown`, and the task took a
/// ladder rung. The interrupted run is settled as unknown and verification reruns.
#[test]
fn an_interrupted_verification_is_settled_unknown_and_rerun_by_the_runtime() {
    let (_p, s, _o, _e) = setup();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='VERIFYING',revision=16 WHERE task_id='t'", [])?;
        lifecycle::request_tx(db, "t", "native-verifier", "verify-old", "native_verification_intent",
            &json!({"revision":16,"digest":"d","binding_digest":"d","commands":[["cargo","test"]]}),
            || Ok(json!({"revision":16,"digest":"d","binding_digest":"d"})))?;
        // A finished run and one of an older revision are left alone.
        lifecycle::request_tx(db, "t", "native-verifier", "verify-done", "native_verification_intent",
            &json!({"revision":16,"x":1}), || Ok(json!({"revision":16})))?;
        lifecycle::request_tx(db, "t", "native-verifier-finish", "verify-done", "native_verification_finished",
            &json!({"keys":[]}), || Ok(json!({"settled":true})))?;
        lifecycle::request_tx(db, "t", "native-verifier", "verify-r15", "native_verification_intent",
            &json!({"revision":15}), || Ok(json!({"revision":15})))?;
        assert_eq!(settle_interrupted_verification_tx(db, "t")?, 1);
        assert_eq!(settle_interrupted_verification_tx(db, "t")?, 0);
        let finish: String = db.query_row(
            "SELECT result_json FROM runtime_requests WHERE task_id='t' AND actor='native-verifier-finish' AND request_key='verify-old'",
            [], |r| r.get(0))?;
        let finish: Value = serde_json::from_str(&finish)?;
        assert_eq!(finish["outcome"], "unknown");
        assert_eq!(finish["settled_by"], "runtime_reverify");
        Ok(())
    })
    .unwrap();
    let reruns = events(&s, "VERIFICATION_RERUN");
    assert_eq!(reruns.len(), 1);
    assert_eq!(reruns[0]["reason"], "outcome_unknown");
    assert_eq!(reruns[0]["revision"], 16);
    assert_eq!(reruns[0]["argv"], json!([["cargo", "test"]]));
    assert_eq!(state(&s), "VERIFYING");
    assert!(events(&s, "VERIFICATION_FAILED").is_empty());
}

/// cortex t_5715 293620-293623: a passing command's output never reached its end,
/// the verification became an unknown outcome and the next continuation called the
/// senior consultant. An environmental failure that survives the runtime's rerun is
/// the runtime's own wait, rechecked after 1, 2, 5 and 10 minutes: no replan, no rung,
/// no passed record. Past the 10-minute recheck the runtime asks the owner one
/// question (no model call) naming the command and the reason in plain words.
fn exhaust_environment_waits(s: &Store) -> String {
    let argv = json!(["scripts/cortex-xtask.sh", "test-fast", "--only", "cargo"]);
    let ru = s.read(|db| Ok(lifecycle::domain::task_language_tx(db, "t")? == "ru")).unwrap();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id='t'",
            [],
        )?;
        assert!(verification_wait_tx(db, "t")?.is_none());
        let first = record_verification_wait_tx(
            db,
            "t",
            &argv,
            "output_unavailable",
            &json!(["native-command:cbd2ab0f", "native-command:rerun"]),
            1000.0,
        )?;
        assert_eq!(first["attempt"], 1);
        assert_eq!(first["retry_not_before"], 1060.0);
        for (n, delay) in [(2, 120), (3, 300), (4, 600)] {
            let w =
                record_verification_wait_tx(db, "t", &argv, "output_unavailable", &json!([]), 0.0)?;
            assert_eq!(w["attempt"], n);
            assert_eq!(w["retry_after_s"], delay);
            assert!(w["owner_question"].is_null());
        }
        assert_eq!(
            verification_wait_tx(db, "t")?.unwrap()["class"],
            "verifier_environment"
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(state(s), "VERIFYING");
    assert!(events(s, "USER_QUESTION_REQUESTED").is_empty());
    // The 10-minute recheck failed the same way: one question, no more waiting.
    let asked = s
        .write(|db| {
            record_verification_wait_tx(db, "t", &argv, "output_unavailable", &json!([]), 0.0)
        })
        .unwrap();
    assert_eq!(asked["attempt"], 5);
    assert!(asked["retry_not_before"].is_null());
    let question = asked["owner_question"].as_str().unwrap().to_owned();
    assert_eq!(state(s), "WAITING_USER");
    let (text, options): (String, String) = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT text,options_json FROM pending_questions WHERE id=? AND status='open'",
                [&question],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert!(
        text.contains("`scripts/cortex-xtask.sh test-fast --only cargo`"),
        "{text}"
    );
    if ru {
        assert!(text.contains("итоговая проверка"), "{text}");
        assert!(text.contains("Среда выполнения автоматически повторила проверку через 1, 2, 5 и 10 минут"), "{text}");
    } else {
        assert!(text.contains("its output never reached its end"), "{text}");
        assert!(text.contains("The runtime itself ran it again after 1, 2, 5 and 10 minutes"), "{text}");
    }
    assert_eq!(
        serde_json::from_str::<Value>(&options).unwrap(),
        {
        let labels = if ru {
            json!([
                "Повторить сейчас: начать новый круг всех стратегий",
                "Другой подход: опишите в ответе, что сделать иначе",
                "Принять без проверки: отметить как непроверенное и завершить задачу",
                "Повторить позже: автоматически через 30 минут",
            ])
        } else {
            json!([
                "Retry now: start a new round of every strategy",
                "Another approach: describe in your answer what to do differently",
                "Accept unverified: record this as not verified and finish the task",
                "Retry later: try again automatically in 30 minutes",
            ])
        };
        let mut expected = labels.as_array().unwrap().iter().map(|l| json!({"label":l})).collect::<Vec<_>>();
        expected[3]["recommended"] = json!(true);
        json!(expected)
    },
    );
    let requested = events(s, "USER_QUESTION_REQUESTED");
    assert_eq!(requested.len(), 1);
    assert_eq!(
        requested[0]["option_ids"],
        json!(lifecycle::automatic::LOOP_OPTIONS)
    );
    assert_eq!(requested[0]["recommended"], 4);
    assert!(events(s, "VERIFICATION_FAILED").is_empty());
    assert!(events(s, "VERIFICATION_PASSED").is_empty());
    assert!(s
        .read(|db| verification_wait_tx(db, "t"))
        .unwrap()
        .is_none());
    question
}
#[test]
fn russian_verification_environment_question_and_disclosure_use_effective_language() {
    let (_p, s, _, _) = setup();
    s.write(|db| {
        lifecycle::event_tx(db, "t", "NATIVE_START_ADMITTED", json!({"goal":"Исправить проверку"}))?;
        lifecycle::event_tx(db, "t", "NATIVE_TASK_SETTINGS", json!({"settings":{"language":"auto"}}))?;
        Ok(())
    }).unwrap();
    let question = exhaust_environment_waits(&s);
    let owed = answer(&s, &question, "Принять без проверки: отметить как непроверенное и завершить задачу");
    assert_eq!(owed.evidence["owner_choice"], "accept_unverified");
    let broken = events(&s, "LOOP_BROKEN");
    let limit = &broken.last().unwrap()["limits"][0];
    assert_eq!(limit["outcome"], "unverified");
    assert!(limit["blocker"].as_str().unwrap().contains(&format!("(вопрос {question})")));
    assert!(events(&s, "VERIFICATION_PASSED").is_empty());
}

fn answer(s: &Store, question: &str, text: &str) -> lifecycle::automatic::Owed {
    s.write(|db| {
        assert!(lifecycle::waits::answer_tx(
            db, "t", question, text, "text", None
        )?);
        let owed = lifecycle::automatic::owed(db, "t")?.unwrap();
        lifecycle::automatic::resume_tx(db, "t", owed.event)?.unwrap();
        Ok(owed)
    })
    .unwrap()
}
/// Another approach: the coordinator changes the verification procedure (a loopback
/// service the verifier sandbox can never reach), and the new procedure decides.
#[test]
fn after_the_ten_minute_recheck_the_owner_is_asked_once_and_another_approach_changes_the_command() {
    let (_p, s, o, e) = setup();
    let question = exhaust_environment_waits(&s);
    let owed = answer(&s, &question,
        "Another approach: describe in your answer what to do differently\n\nRun the unit tests only");
    assert_eq!(owed.evidence["owner_choice"], "other_approach");
    let instruction = owed.evidence["instruction"].as_str().unwrap();
    assert!(instruction.contains("verification_submit"), "{instruction}");
    assert!(
        instruction.contains("Run the unit tests only"),
        "{instruction}"
    );
    assert_eq!(state(&s), "PLANNING");
    s.write(|db| {
        submit_verification_tx(
            db,
            "t",
            "coordinator",
            "changed",
            &json!({"verification":[["cargo","test"]]}),
        )
    })
    .unwrap();
    assert_eq!(state(&s), "VERIFYING");
    verify(&s, &o, &e, "changed", 0);
    assert_eq!(state(&s), "RECONCILING");
    assert_eq!(events(&s, "USER_QUESTION_REQUESTED").len(), 1);
    assert!(events(&s, "AUTOMATIC_CONSULTATION_REQUESTED").is_empty());
}
/// Accept unverified: the command becomes a disclosed limit (Not verified), never a
/// pass, and the rest of the verification completes the task.
#[test]
fn an_accepted_environment_failure_is_a_disclosed_limit_never_a_pass() {
    let (_p, s, o, mut e) = setup();
    e.artifacts.insert("r".into(), "f".repeat(64));
    let question = exhaust_environment_waits(&s);
    let owed = answer(&s, &question, "3");
    assert_eq!(owed.evidence["owner_choice"], "accept_unverified");
    s.read(|db| {
        assert_eq!(
            lifecycle::automatic::owner_accepted_commands(db, "t")?,
            vec![json!([
                "scripts/cortex-xtask.sh",
                "test-fast",
                "--only",
                "cargo"
            ])]
        );
        let gaps = gaps(db, "t")?;
        assert!(
            gaps.values()
                .any(|g| g.starts_with("Accepted without verification")),
            "{gaps:?}"
        );
        Ok(())
    })
    .unwrap();
    s.write(|db| {
        submit_verification_tx(
            db,
            "t",
            "coordinator",
            "rest",
            &json!({"verification":[["cargo","test"]]}),
        )
    })
    .unwrap();
    verify(&s, &o, &e, "rest", 0);
    let limit = s
        .read(|db| gaps(db, "t"))
        .unwrap()
        .into_values()
        .next()
        .unwrap();
    assert_eq!(
        s.write(|db| submit_synthesis_tx(db, "t", &json!({"outcomes":[{"id":"goal","status":"done","evidence":"r"}],"text":"Done"})))
            .unwrap_err()
            .code(),
        "synthesis_gaps_undisclosed"
    );
    s.write(|db| {
        submit_synthesis_tx(
            db,
            "t",
            &json!({"outcomes":[{"id":"goal","status":"done","evidence":"r"}],"text":"Done",
        "not_verified":[{"item":limit,"blocker":"Accepted by the owner without verification."}]}),
        )
    })
    .unwrap();
    s.write(|db| complete_tx(db, "t", &e)).unwrap();
    assert_eq!(state(&s), "COMPLETED");
    assert!(events(&s, "VERIFICATION_PASSED").iter().all(|p| p["checks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["argv"] != json!(["scripts/cortex-xtask.sh", "test-fast", "--only", "cargo"]))));
}
/// Retry now: the coordinator resubmits the same procedure and the runtime waits and
/// reruns from the start again; the same question is asked again only after that.
#[test]
fn retry_now_reruns_the_same_verification_with_a_fresh_wait() {
    let (_p, s, _o, _e) = setup();
    let question = exhaust_environment_waits(&s);
    let owed = answer(&s, &question, "retry_now");
    assert_eq!(owed.evidence["owner_choice"], "retry_now");
    let same = s.read(|db| Ok(db.query_row("SELECT json_extract(plan_json,'$.verification') FROM runtime_tasks WHERE task_id='t'",
        [], |r| r.get::<_, String>(0))?)).unwrap();
    s.write(|db| {
        submit_verification_tx(
            db,
            "t",
            "coordinator",
            "again",
            &json!({"verification":serde_json::from_str::<Value>(&same).unwrap()}),
        )
    })
    .unwrap();
    s.write(|db| {
        let w = record_verification_wait_tx(
            db,
            "t",
            &json!(["cargo", "test"]),
            "output_unavailable",
            &json!([]),
            0.0,
        )?;
        assert_eq!(w["attempt"], 1);
        Ok(())
    })
    .unwrap();
}

/// Defect 15: the same failure came back on every rung with nothing telling the
/// ladder it was the same. Every failed verification carries a normalized
/// `failure_key` (argv, exit code, normalized output tail), equal for identical
/// failures across runs and resumes and different for a different failure, which
/// the ladder's per-fingerprint rung choice uses. A rung's own direct repairs stay:
/// the first failure after a resume still returns to planning.
#[test]
fn identical_verification_failures_share_a_failure_key_across_rungs() {
    let (_p, s, o, e) = setup();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id='t'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    verify(&s, &o, &e, "f1", 1);
    assert_eq!(state(&s), "PLANNING");
    reverify(&s, false);
    verify(&s, &o, &e, "f2", 1);
    reverify(&s, true);
    verify(&s, &o, &e, "f3", 1);
    assert_eq!(
        state(&s),
        "PLANNING",
        "a resumed rung keeps its direct repairs"
    );
    reverify(&s, false);
    verify(&s, &o, &e, "f4", 2);
    let failed = events(&s, "VERIFICATION_FAILED");
    let keys: Vec<&str> = failed
        .iter()
        .map(|f| f["failure_key"].as_str().unwrap())
        .collect();
    assert_eq!(keys[0], keys[1]);
    assert_eq!(keys[0], keys[2]);
    assert_ne!(keys[0], keys[3], "another exit is another failure");
    assert_eq!(keys[0].len(), 64);
}

/// Defect 14: the coordinator could resubmit the identical verification on an
/// unchanged project forever. One identical resubmission (a possible flake) is
/// accepted; another with nothing changed since is refused `verification_unchanged`.
#[test]
fn an_identical_verification_on_unchanged_work_is_refused_after_one_rerun() {
    let (_p, s, o, e) = setup();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id='t'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let same = json!({"verification":[["cargo","test"]]});
    verify(&s, &o, &e, "first", 1);
    s.write(|db| submit_verification_tx(db, "t", "coordinator", "again-1", &same))
        .unwrap();
    verify(&s, &o, &e, "second", 1);
    assert_eq!(
        s.write(|db| submit_verification_tx(db, "t", "coordinator", "again-2", &same))
            .unwrap_err()
            .code(),
        "verification_unchanged"
    );
    // A worker command since (the environment may have changed) allows it again.
    s.write(|db| {
        lifecycle::event_tx(
            db,
            "t",
            "NATIVE_COMMAND_STARTED",
            json!({"ref":"native-command:x","channel_id":"ch_1","argv":["cargo","fetch"]}),
        )?;
        Ok(())
    })
    .unwrap();
    s.write(|db| submit_verification_tx(db, "t", "coordinator", "again-3", &same))
        .unwrap();
    assert_eq!(state(&s), "VERIFYING");
}

fn packet(path_sha: &[(&str, &str)], content: Option<&str>) -> Value {
    let files: Vec<Value> = path_sha
        .iter()
        .map(|(path, sha)| {
            let mut f =
                json!({"path":path,"change":"modified","current_sha256":sha,"required":true});
            if let Some(kind) = content {
                f["review_content"] = json!(kind);
            }
            f
        })
        .collect();
    json!({"changed_files":{"complete":true,"attribution_complete":true,"files":files,"total":files.len()},
        "read_scope_covers_change":true,"outside_read_scope":[]})
}
fn review(s: &Store, assignment: &str, packet: Value) {
    s.write(|db| {
        lifecycle::event_tx(db, "t", "REVIEW_PACKET",
            json!({"assignment_ref":assignment,"node":"_review_1","packet":packet,"binding":{"kind":"checkout"}}))?;
        Ok(())
    })
    .unwrap();
}
fn refuse_coverage(s: &Store, assignment: &str, paths: &[&str]) {
    let missing: Vec<String> = paths
        .iter()
        .map(|p| format!("{p} (current complete read is missing or stale)"))
        .collect();
    s.write(|db| {
        lifecycle::event_tx(
            db,
            "t",
            "WORKER_RESULT_REFUSED",
            json!({"assignment_ref":assignment,"attempt":"xa_1","channel_id":"ch_1",
                "code":"review_file_coverage_required","missing":missing}),
        )?;
        Ok(())
    })
    .unwrap();
}
fn missing(s: &Store, assignment: &str) -> Vec<String> {
    s.read(|db| review_coverage_missing_tx(db, "t", assignment, &json!({"verdict":"APPROVE"})))
        .unwrap()
        .iter()
        .map(|m| m["path"].as_str().unwrap().to_owned())
        .collect()
}

/// AI-Frontier t_5f6443 327676-328670: three review assignments across revisions
/// (`_review_1`, `_review_2`, `_review_1` of a later plan) were each refused twice
/// for the same 32 files, because the refusal cap counted per assignment. The count
/// is per review lineage and file bytes: settled bytes are never required again and
/// are disclosed as unread; changed bytes must be read.
#[test]
fn coverage_refusals_count_per_review_lineage_and_file_bytes() {
    let (_p, s, _o, _e) = setup();
    let files = [
        (
            ".playwright-mcp/page-2026-11-21T20-35-54-419Z.yml",
            "1".repeat(64),
        ),
        ("apps/web/app/[locale]/my-feed/page.tsx", "2".repeat(64)),
        ("apps/web/components/account/parts.tsx", "3".repeat(64)),
    ];
    let first: Vec<(&str, &str)> = files.iter().map(|(p, h)| (*p, h.as_str())).collect();
    let paths: Vec<&str> = files.iter().map(|(p, _)| *p).collect();
    review(&s, "72500a28", packet(&first, None));
    assert_eq!(missing(&s, "72500a28"), paths);
    refuse_coverage(&s, "72500a28", &paths);
    assert_eq!(
        missing(&s, "72500a28"),
        paths,
        "one refusal is a chance to read"
    );
    refuse_coverage(&s, "72500a28", &paths[..2]);
    // A reassignment in a later revision: parts.tsx changed bytes, the others did not.
    let changed = "4".repeat(64);
    let second = vec![first[0], first[1], (paths[2], changed.as_str())];
    review(&s, "4d20456a", packet(&second, None));
    assert_eq!(missing(&s, "4d20456a"), vec![paths[2]]);
    let limits = s
        .read(|db| review_change_set_limits_tx(db, "t", "4d20456a"))
        .unwrap();
    let unread = limits
        .iter()
        .find(|l| l["name"] == "Changed files the reviewer did not read")
        .unwrap();
    assert_eq!(unread["outcome"], "unverified");
    assert_eq!(unread["paths"], json!([paths[0], paths[1]]));
    // A file an approval already disclosed as unread is never required again for
    // the same bytes, even without two refusals.
    s.write(|db| {
        lifecycle::event_tx(db, "t", "REVIEW_CHANGE_SET_LIMITED",
            json!({"assignment_ref":"4d20456a","limits":[{"name":"Changed files the reviewer did not read",
                "outcome":"unverified","paths":[paths[2]],"source":"runtime"}]}))?;
        Ok(())
    })
    .unwrap();
    review(&s, "c5caccb4", packet(&second, None));
    assert!(missing(&s, "c5caccb4").is_empty());
    // A reopen starts over.
    s.write(|db| {
        lifecycle::event_tx(db, "t", "TASK_REOPENED", json!({}))?;
        Ok(())
    })
    .unwrap();
    review(&s, "after", packet(&second, None));
    assert_eq!(missing(&s, "after").len(), 3);
}

/// Large generated data (a ~66 KiB Playwright page snapshot, about twenty pages) is
/// `bulk`: never a missing read, disclosed as not reviewed in full. An SVG stays an
/// image and keeps its own limit.
#[test]
fn bulk_and_image_files_are_disclosed_limits_not_required_reads() {
    let (_p, s, _o, _e) = setup();
    let bulk = [(
        ".playwright-mcp/page-2026-11-21T21-33-26-671Z.yml",
        "5".repeat(64),
    )];
    let bulk: Vec<(&str, &str)> = bulk.iter().map(|(p, h)| (*p, h.as_str())).collect();
    review(&s, "bulk", packet(&bulk, Some("bulk")));
    assert!(missing(&s, "bulk").is_empty());
    let limits = s
        .read(|db| review_change_set_limits_tx(db, "t", "bulk"))
        .unwrap();
    assert_eq!(limits.len(), 1, "{limits:?}");
    assert_eq!(
        limits[0]["name"],
        "Changed files too large to require a complete read"
    );
    assert_eq!(limits[0]["paths"], json!([bulk[0].0]));
    let image = [("assets/part.svg", "6".repeat(64))];
    let image: Vec<(&str, &str)> = image.iter().map(|(p, h)| (*p, h.as_str())).collect();
    review(&s, "image", packet(&image, Some("image")));
    assert!(missing(&s, "image").is_empty());
    let limits = s
        .read(|db| review_change_set_limits_tx(db, "t", "image"))
        .unwrap();
    assert_eq!(limits[0]["name"], "Changed files without reviewable text");
}

/// AI-Frontier t_5f6443 327700-328740 with task_events 205126: an owner message the
/// former runtime stored on 2026-10-05 was never consumed, so three passed
/// verifications each ended in `completion_pending_input` and a ladder rung. A
/// message a later coordinator turn carried in its capsule is delivered; one no
/// coordinator has seen yet still holds completion and is named.
#[test]
fn a_former_runtime_message_a_coordinator_already_carried_no_longer_holds_completion() {
    let (_p, s, o, mut e) = setup();
    e.artifacts.insert("r".into(), "f".repeat(64));
    s.write(|db| {
        db.execute("INSERT INTO task_events(task_id,event_type,dedup_key,payload_json,origin,created_at,consumed_by,superseded)
            VALUES ('t','user_message','legacy-1',?,'user','2026-10-05T00:35:29.755800+00:00',NULL,0)",
            [json!({"content":"No left borders anywhere in the project.","session_id":"acps_9c816c4181d7"}).to_string()])?;
        db.execute("UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id='t'", [])?;
        Ok(())
    })
    .unwrap();
    s.write(|db| {
        let items = pending_input_tx(db, "t")?;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["kind"], "undelivered_message");
        assert_eq!(items[0]["recorded_at"], "2026-10-05T00:35:29.755800+00:00");
        // No coordinator turn carried it yet.
        assert_eq!(settle_delivered_legacy_input_tx(db, "t")?, 0);
        Ok(())
    })
    .unwrap();
    verify(&s, &o, &e, "pass", 0);
    s.write(|db| submit_synthesis_tx(db, "t", &json!({"outcomes":[{"id":"goal","status":"done","evidence":"r"}],"text":"Done"})))
        .unwrap();
    // The synthesis was a coordinator turn after the message: delivered.
    s.write(|db| complete_tx(db, "t", &e)).unwrap();
    assert_eq!(state(&s), "COMPLETED");
    let consumed: Option<String> = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT consumed_by FROM task_events WHERE dedup_key='legacy-1'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(consumed.as_deref(), Some("runtime:recovery_capsule"));
}
#[test]
fn a_former_runtime_message_newer_than_every_coordinator_turn_is_named() {
    let (_p, s, o, mut e) = setup();
    e.artifacts.insert("r".into(), "f".repeat(64));
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id='t'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    verify(&s, &o, &e, "pass", 0);
    s.write(|db| submit_synthesis_tx(db, "t", &json!({"outcomes":[{"id":"goal","status":"done","evidence":"r"}],"text":"Done"})))
        .unwrap();
    s.write(|db| {
        db.execute("INSERT INTO task_events(task_id,event_type,dedup_key,payload_json,origin,created_at,consumed_by,superseded)
            VALUES ('t','user_message','late',?,'user','2999-01-01T00:00:00.000000+00:00',NULL,0)",
            [json!({"content":"One more thing."}).to_string()])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.write(|db| complete_tx(db, "t", &e)).unwrap_err().code(),
        "completion_pending_input"
    );
    let items = s.write(|db| pending_input_tx(db, "t")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["source"], "task_events");
}

#[test]
fn a_launcher_names_the_program_it_runs() {
    let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        launched_program(&argv(&["timeout", "900", "cargo", "build", "--locked"])),
        Some("cargo")
    );
    assert_eq!(
        launched_program(&argv(&["env", "-C", "e2e", "node", "cli.js"])),
        Some("node")
    );
    assert_eq!(launched_program(&argv(&["cargo", "test"])), None);
    assert_eq!(
        launched_program(&argv(&["env", "PATH=/opt/bin", "cargo"])),
        None
    );
}

/// A limit disclosed when the runtime broke a loop on a node of an earlier plan
/// revision (`LOOP_BROKEN.limits`) must still reach the final result's Not verified
/// list after a replan; a reopen starts over.
#[test]
fn a_loop_broken_limit_must_be_disclosed_by_the_final_result() {
    let (_p, s, o, e) = setup();
    s.write(|db| {
        lifecycle::event_tx(db, "t", "LOOP_BROKEN", json!({"key":"old","kind":"result_refusal","action":"disclose",
            "limits":[{"name":"Before reopen","outcome":"unverified","blocker":"x","source":"runtime"}]}))?;
        lifecycle::event_tx(db, "t", "TASK_REOPENED", json!({}))?;
        lifecycle::event_tx(db, "t", "LOOP_BROKEN", json!({"key":"k","kind":"result_refusal","action":"disclose",
            "limits":[{"name":"Coverage of generated fixtures","outcome":"unverified",
                "blocker":"The worker could not read them after repeated refusals.","source":"runtime"}]}))?;
        db.execute("UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id='t'", [])?;
        Ok(())
    })
    .unwrap();
    let gaps = s.read(|db| gaps(db, "t")).unwrap();
    assert_eq!(
        gaps.values().collect::<Vec<_>>(),
        vec!["Coverage of generated fixtures"]
    );
    verify(&s, &o, &e, "pass", 0);
    assert_eq!(
        s.write(|db| submit_synthesis_tx(db, "t", &json!({"outcomes":[{"id":"goal","status":"done","evidence":"r"}],"text":"Done"})))
            .unwrap_err()
            .code(),
        "synthesis_gaps_undisclosed"
    );
    s.write(|db| submit_synthesis_tx(db, "t", &json!({"outcomes":[{"id":"goal","status":"done","evidence":"r"}],"text":"Done","not_verified":[
        {"item":"Coverage of generated fixtures","blocker":"The worker could not read them after repeated refusals."}]})))
        .unwrap();
}

fn verify_argv(
    s: &Store,
    o: &OwnerEpoch,
    e: &EvidenceSnapshot,
    key: &str,
    argv: &[&str],
    code: i32,
) {
    let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
    s.write(|db| {
        retain_check_tx(
            db,
            "t",
            o,
            key,
            &argv,
            &CommandObservation {
                complete: true,
                exit_code: Some(code),
                receipt_ref: key.into(),
            },
            e,
        )?;
        record_verification_tx(db, "t", &[key.into()], e)?;
        Ok(())
    })
    .unwrap();
}
/// The owner accepted a failing command as a disclosed unverified limit after a
/// loop question (`LOOP_BROKEN` action `accept_unverified`): the repair may leave it
/// out and the task completes, with the limit under Not verified and never a pass.
/// Without that acceptance the dropped command still holds completion.
#[test]
fn an_owner_accepted_failing_command_completes_with_its_limit_disclosed() {
    for accepted in [false, true] {
        let (_p, s, o, mut e) = setup();
        e.artifacts.insert("r".into(), "f".repeat(64));
        verify_argv(&s, &o, &e, "failed", &["cargo", "test"], 1);
        assert_eq!(state(&s), "PLANNING");
        let limit = json!({"name":"cargo test","outcome":"unverified",
            "blocker":"The owner accepted this failing check as not verified.","source":"runtime"});
        if accepted {
            s.write(|db| {
                lifecycle::event_tx(db, "t", "LOOP_BROKEN", json!({"key":"k","kind":"verification_failure",
                    "action":"accept_unverified","limits":[limit],"accepted_commands":[["cargo","test"]]}))?;
                Ok(())
            })
            .unwrap();
        }
        let repair = json!({"verification":[["cargo","check"]]});
        s.write(|db| submit_verification_tx(db, "t", "coordinator", "repair", &repair))
            .unwrap();
        verify_argv(&s, &o, &e, "check", &["cargo", "check"], 0);
        assert_eq!(state(&s), "RECONCILING");
        if accepted {
            assert_eq!(
                s.write(|db| submit_synthesis_tx(db, "t", &json!({"outcomes":[{"id":"goal","status":"done","evidence":"r"}],"text":"Done"})))
                    .unwrap_err()
                    .code(),
                "synthesis_gaps_undisclosed"
            );
        }
        s.write(|db| submit_synthesis_tx(db, "t", &json!({"outcomes":[{"id":"goal","status":"done","evidence":"r"}],"text":"Done","not_verified":[
            {"item":"cargo test","blocker":"The owner accepted this failing check as not verified."}]})))
            .unwrap();
        let completed = s.write(|db| complete_tx(db, "t", &e));
        if accepted {
            completed.unwrap();
            assert_eq!(state(&s), "COMPLETED");
            // Never recorded as passed.
            let passed = events(&s, "VERIFICATION_PASSED");
            assert!(passed.iter().all(|p| p["checks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["argv"] != json!(["cargo", "test"]))));
        } else {
            assert_eq!(
                completed.unwrap_err().code(),
                "completion_dropped_failed_check"
            );
        }
    }
}

/// The owner accepted the plan's only verification command as not verified: the
/// empty procedure is accepted, nothing is run or recorded as passed, and the task
/// completes with the limit under Not verified. With another command still in the
/// plan an empty procedure is refused.
#[test]
fn every_command_accepted_unverified_completes_without_a_passed_verification() {
    let (_p, s, _o, mut e) = setup();
    e.artifacts.insert("r".into(), "f".repeat(64));
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET plan_json=json_set(plan_json,'$.verification',json(?)) WHERE task_id='t'",
            [json!([["scripts/cortex-xtask.sh", "test-fast", "--only", "cargo"]]).to_string()])?;
        Ok(())
    })
    .unwrap();
    let question = exhaust_environment_waits(&s);
    answer(&s, &question, "accept_unverified");
    let empty = json!({"verification":[]});
    let reply = s
        .write(|db| submit_verification_tx(db, "t", "coordinator", "none-left", &empty))
        .unwrap();
    assert_eq!(reply["state"], "RECONCILING");
    assert_eq!(state(&s), "RECONCILING");
    let limit = s
        .read(|db| gaps(db, "t"))
        .unwrap()
        .into_values()
        .next()
        .unwrap();
    s.write(|db| {
        submit_synthesis_tx(
            db,
            "t",
            &json!({"outcomes":[{"id":"goal","status":"done","evidence":"r"}],"text":"Done",
        "not_verified":[{"item":limit,"blocker":"Accepted by the owner without verification."}]}),
        )
    })
    .unwrap();
    s.write(|db| complete_tx(db, "t", &e)).unwrap();
    assert_eq!(state(&s), "COMPLETED");
    assert!(events(&s, "VERIFICATION_PASSED").is_empty());
    assert_eq!(
        events(&s, "VERIFICATION_REVISED").last().unwrap()["accepted_unverified"],
        json!([["scripts/cortex-xtask.sh", "test-fast", "--only", "cargo"]])
    );

    // Another command still in the plan: an empty procedure is refused.
    let (_p2, s2, _o2, _e2) = setup();
    s2.write(|db| {
        db.execute("UPDATE runtime_tasks SET plan_json=json_set(plan_json,'$.verification',json(?)) WHERE task_id='t'",
            [json!([["scripts/cortex-xtask.sh", "test-fast", "--only", "cargo"], ["cargo", "test"]]).to_string()])?;
        Ok(())
    })
    .unwrap();
    let question = exhaust_environment_waits(&s2);
    answer(&s2, &question, "accept_unverified");
    assert_eq!(
        s2.write(|db| submit_verification_tx(db, "t", "coordinator", "none-left", &empty))
            .unwrap_err()
            .code(),
        "verification_required"
    );
}
