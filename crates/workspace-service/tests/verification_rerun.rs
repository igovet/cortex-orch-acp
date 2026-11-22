//! The runtime's own verification through the real driver and the real read-only
//! verifier sandbox, with fake commands (POSIX shell scripts in the project):
//! an environmental outcome is rerun once by the runtime, a persisting one is the
//! runtime's own wait (no replan, no model call) rechecked later, a real failure stays
//! a failure, and an interrupted run is settled unknown and rerun.
#[path = "support/hermetic.rs"]
mod hermetic;

use cortex_core::{ownership::OwnerEpoch, OpenMode, ProjectKey, Store, StoreChannel};
use serde_json::{json, Value};
use std::{os::unix::fs::DirBuilderExt, path::PathBuf};
use workspace_service::native_sessions::NativeHost;

struct Project(PathBuf);
impl Project {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("verification-rerun-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        assert!(std::process::Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&path)
            .status()
            .unwrap()
            .success());
        // `state/` is git-ignored: the verifier may write there, and a change there
        // is not a change of the reviewed checkout.
        std::fs::write(path.join(".gitignore"), "state/\n").unwrap();
        std::fs::create_dir(path.join("state")).unwrap();
        std::fs::write(
            path.join("check.sh"),
            "case \"$1\" in\n\
             flaky) if [ -e state/ran ]; then exit 0; fi; : > state/ran; exit 127;;\n\
             missing) if [ -e state/ok ]; then exit 0; fi; echo 'cargo: not found' >&2; exit 127;;\n\
             fail) echo 'assertion failed: left == right'; exit 1;;\n\
             *) exit 0;;\n\
             esac\n",
        )
        .unwrap();
        Self(path)
    }
    fn key(&self) -> ProjectKey {
        ProjectKey::new(&self.0, StoreChannel::Dev).unwrap()
    }
    fn store(&self) -> Store {
        Store::open(self.key(), OpenMode::RuntimeValidate).unwrap()
    }
    async fn host(&self) -> NativeHost {
        let key = self.key();
        tokio::task::spawn_blocking(move || NativeHost::open(key))
            .await
            .unwrap()
            .unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn close(host: NativeHost) {
    tokio::task::spawn_blocking(move || drop(host))
        .await
        .unwrap();
}

/// A task whose single plan node is already complete and whose project did not
/// change since its baseline: the driver records the baseline, needs no review
/// (`REVIEW_NOT_REQUIRED`) and runs the plan's verification.
async fn verifying(p: &Project, mode: &str) -> (NativeHost, OwnerEpoch, Store) {
    let h = p.host().await;
    let o = h
        .create("t".into(), "Verify fixture".into(), "owner".into())
        .await
        .unwrap();
    let s = p.store();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLAN_READY',revision=1,plan_json=? WHERE task_id='t'",
            [json!({"tasks":[],"verification":[["/bin/sh","check.sh",mode]]}).to_string()])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'work','worker-backend-dev','Work','[]','[\"src\"]','execution','completed','a',?)",
            [json!({"report_ref":"r","report_digest":"f".repeat(64),"checks":[]}).to_string()])?;
        Ok(())
    })
    .unwrap();
    // The baseline step: PLAN_READY to EXECUTING.
    let step = h.advance("t".into(), o.clone()).await.unwrap();
    assert_eq!(step["state"], "EXECUTING", "{step}");
    (h, o, s)
}
fn count(s: &Store, kind: &str) -> i64 {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id='t' AND type=?",
            [kind],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}
fn latest(s: &Store, kind: &str) -> Value {
    s.read(|db| {
        let raw: String = db.query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id='t' AND type=? ORDER BY id DESC LIMIT 1",
            [kind],
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str(&raw)?)
    })
    .unwrap()
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
/// Whether the read-only verifier sandbox could run anything here: an unavailable
/// sandbox is reported and the scenario is not run, never treated as a pass.
fn sandbox_ran(s: &Store) -> bool {
    let unavailable: bool = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE operation='native_command_output'
                 AND json_extract(result_json,'$.observation.failure')='native_verification_sandbox_unavailable')",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    if unavailable {
        println!("native verification sandbox unavailable: scenario not run");
    }
    !unavailable
}

/// cortex t_e308 274436/274464: a bare program the verifier did not find (exit 127)
/// failed verification and replanned. The runtime reruns it once itself; the rerun's
/// pass counts.
#[tokio::test]
async fn an_environmental_outcome_is_rerun_once_and_the_rerun_counts() {
    let p = Project::new("flaky");
    let (h, o, s) = verifying(&p, "flaky").await;
    reconcile(&h, &o).await;
    if sandbox_ran(&s) {
        assert_eq!(count(&s, "VERIFICATION_RERUN"), 1);
        let rerun = latest(&s, "VERIFICATION_RERUN");
        assert_eq!(rerun["reason"], "program_not_found");
        assert_eq!(rerun["argv"], json!(["/bin/sh", "check.sh", "flaky"]));
        assert_eq!(count(&s, "VERIFICATION_PASSED"), 1);
        assert_eq!(count(&s, "VERIFICATION_FAILED"), 0);
        let passed = latest(&s, "VERIFICATION_PASSED");
        assert_eq!(passed["checks"][0]["exit_code"], 0);
        // Both runs are retained; only the rerun is the check.
        let outputs: i64 = s
            .read(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM runtime_requests WHERE operation='native_command_output'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(outputs, 2);
    }
    close(h).await;
}

/// The same environmental failure after the rerun is the runtime's own wait: the
/// task stays VERIFYING with no failure recorded, nothing runs before the recheck is
/// due, and the due recheck verifies once the environment is back.
#[tokio::test]
async fn a_persisting_environmental_failure_waits_and_is_rechecked_without_a_replan() {
    let p = Project::new("missing");
    let (h, o, s) = verifying(&p, "missing").await;
    let step = h.advance("t".into(), o.clone()).await.unwrap();
    if sandbox_ran(&s) {
        assert_eq!(step["verificationWait"], true, "{step}");
        assert_eq!(step["state"], "VERIFYING");
        assert_eq!(state(&s), "VERIFYING");
        assert_eq!(count(&s, "VERIFICATION_RERUN"), 1);
        assert_eq!(count(&s, "VERIFICATION_FAILED"), 0);
        assert_eq!(count(&s, "AUTOMATIC_CONTINUATION"), 0);
        let wait = latest(&s, "VERIFICATION_ENVIRONMENT_WAIT");
        assert_eq!(wait["reason"], "program_not_found");
        assert_eq!(wait["attempt"], 1);
        assert_eq!(wait["receipts"].as_array().unwrap().len(), 2);
        // Before the recheck is due nothing runs.
        let started = count(&s, "NATIVE_COMMAND_STARTED");
        let again = h.advance("t".into(), o.clone()).await.unwrap();
        assert_eq!(again["verificationWait"], true);
        assert_eq!(count(&s, "NATIVE_COMMAND_STARTED"), started);
        // The environment is back and the recheck is due: one run, no second rerun.
        std::fs::write(p.0.join("state/ok"), "").unwrap();
        s.write(|db| {
            db.execute("UPDATE runtime_events SET payload_json=json_set(payload_json,'$.retry_not_before',0)
                WHERE task_id='t' AND type='VERIFICATION_ENVIRONMENT_WAIT'", [])?;
            Ok(())
        })
        .unwrap();
        reconcile(&h, &o).await;
        assert_eq!(count(&s, "NATIVE_COMMAND_STARTED"), started + 1);
        assert_eq!(count(&s, "VERIFICATION_RERUN"), 1);
        assert_eq!(count(&s, "VERIFICATION_PASSED"), 1);
        assert_eq!(count(&s, "VERIFICATION_FAILED"), 0);
    }
    close(h).await;
}

/// Past the 10-minute recheck the runtime stops waiting and asks the owner one
/// question (no model call) naming the command and the reason; nothing is recorded
/// as failed or passed.
#[tokio::test]
async fn after_the_ten_minute_recheck_the_owner_is_asked_once() {
    let p = Project::new("asked");
    let (h, o, s) = verifying(&p, "missing").await;
    h.advance("t".into(), o.clone()).await.unwrap();
    if sandbox_ran(&s) {
        // The 1, 2 and 5 minute rechecks were spent; the 10-minute one is due.
        s.write(|db| {
            db.execute("UPDATE runtime_events SET payload_json=json_set(payload_json,'$.attempt',4,'$.retry_not_before',0)
                WHERE task_id='t' AND type='VERIFICATION_ENVIRONMENT_WAIT'", [])?;
            Ok(())
        })
        .unwrap();
        let step = h.advance("t".into(), o.clone()).await.unwrap();
        assert_eq!(step["state"], "WAITING_USER", "{step}");
        assert_eq!(state(&s), "WAITING_USER");
        assert_eq!(count(&s, "USER_QUESTION_REQUESTED"), 1);
        assert_eq!(count(&s, "VERIFICATION_FAILED"), 0);
        assert_eq!(count(&s, "VERIFICATION_PASSED"), 0);
        let question = latest(&s, "USER_QUESTION_REQUESTED");
        let text = question["question"].as_str().unwrap();
        assert!(text.contains("`/bin/sh check.sh missing`"), "{text}");
        assert!(text.contains("the program is not found there"), "{text}");
        // Waiting for the owner runs nothing more.
        let started = count(&s, "NATIVE_COMMAND_STARTED");
        let _ = h.advance("t".into(), o.clone()).await;
        assert_eq!(count(&s, "NATIVE_COMMAND_STARTED"), started);
        assert_eq!(count(&s, "USER_QUESTION_REQUESTED"), 1);
    }
    close(h).await;
}

/// A nonzero exit with complete output is a real failure: no rerun, no wait.
#[tokio::test]
async fn a_real_failure_stays_a_failure() {
    let p = Project::new("fail");
    let (h, o, s) = verifying(&p, "fail").await;
    h.advance("t".into(), o.clone()).await.unwrap();
    if sandbox_ran(&s) {
        assert_eq!(count(&s, "VERIFICATION_RERUN"), 0);
        assert_eq!(count(&s, "VERIFICATION_ENVIRONMENT_WAIT"), 0);
        assert_eq!(count(&s, "VERIFICATION_FAILED"), 1);
        let failed = latest(&s, "VERIFICATION_FAILED");
        assert!(failed["failure_key"].is_string());
        // What failed travels with the event (and the capsule), since no worker
        // channel can read the runtime's own verifier output.
        assert_eq!(
            failed["diagnostics"][0]["argv"],
            json!(["/bin/sh", "check.sh", "fail"])
        );
        assert!(failed["diagnostics"][0]["excerpt"]
            .as_str()
            .unwrap()
            .contains("assertion failed: left == right"));
        assert_eq!(state(&s), "PLANNING");
    }
    close(h).await;
}

/// cortex t_5715 288531-288538: a run interrupted by a restart left its intent
/// unfinished and every later verification of the revision was refused
/// `native_verification_outcome_unknown`. It is settled unknown and verification
/// runs again.
#[tokio::test]
async fn an_interrupted_run_is_settled_unknown_and_verification_runs_again() {
    let p = Project::new("interrupted");
    let (h, o, s) = verifying(&p, "pass").await;
    s.write(|db| {
        db.execute("INSERT INTO runtime_requests VALUES ('t','verify-interrupted','native-verifier','native_verification_intent','d',?)",
            [json!({"revision":1,"digest":"d","binding_digest":"d"}).to_string()])?;
        Ok(())
    })
    .unwrap();
    reconcile(&h, &o).await;
    if sandbox_ran(&s) {
        let rerun = latest(&s, "VERIFICATION_RERUN");
        assert_eq!(rerun["reason"], "outcome_unknown");
        assert_eq!(rerun["settled_by"], "runtime_reverify");
        let finish: String = s
            .read(|db| Ok(db.query_row("SELECT result_json FROM runtime_requests WHERE actor='native-verifier-finish' AND request_key='verify-interrupted'", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&finish).unwrap()["outcome"],
            "unknown"
        );
        assert_eq!(count(&s, "VERIFICATION_PASSED"), 1);
    }
    close(h).await;
}

/// Advance past a passing verification. The fixture has no published reports, so
/// reconciliation itself refuses (`not_found`) after the verification is recorded;
/// what this file checks is the recorded verification.
async fn reconcile(h: &NativeHost, o: &OwnerEpoch) {
    match h.advance("t".into(), o.clone()).await {
        Ok(_) => {}
        Err(e) => assert_eq!(e.code(), "not_found"),
    }
}
