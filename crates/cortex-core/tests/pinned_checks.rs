//! Runtime-kept verification commands pinned to a file hash follow the accepted
//! plan: a stale pin is retired as superseded, never enforced and never put to
//! the owner.
mod support;
use cortex_core::lifecycle::{event_tx, pinned_checks::retire_superseded_tx};
use cortex_core::lifecycle::TaskMachine;
use serde_json::{json, Value};
use support::*;

const OLD: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const NEW: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn pinned(sha: &str) -> Value {
    json!(["python3","-B","-I","-c",format!(
        "import hashlib,pathlib\nd=pathlib.Path(\"episodes/EP001/storyboard.json\").read_bytes()\nassert hashlib.sha256(d).hexdigest()==\"{sha}\"")])
}

fn setup(plan_verification: Value) -> (Project, cortex_core::Store) {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='VERIFYING',revision=1,plan_json=? WHERE task_id='t'",
            [json!({"tasks":[],"verification":plan_verification}).to_string()],
        )?;
        Ok(())
    })
    .unwrap();
    (p, s)
}
fn verification(s: &cortex_core::Store) -> Value {
    s.read(|db| {
        let p: String = db.query_row("SELECT plan_json FROM runtime_tasks WHERE task_id='t'", [], |r| r.get(0))?;
        Ok(serde_json::from_str::<Value>(&p).unwrap()["verification"].clone())
    })
    .unwrap()
}

/// A check pinned to the old hash passed once; accepted work then changed the file
/// and the check failed. The newer pin to the same file stays; the old one is
/// retired as superseded and recorded.
#[test]
fn a_check_pinned_to_an_old_hash_is_superseded_by_the_accepted_change() {
    let (_p, s) = setup(json!([["cargo","test"], pinned(OLD), pinned(NEW)]));
    s.write(|db| {
        event_tx(db, "t", "PLAN_ACCEPTED", json!({"plan":{"verification":[["cargo","test"], pinned(OLD)]}}))?;
        event_tx(db, "t", "VERIFICATION_PASSED", json!({"checks":[{"argv":pinned(OLD),"complete":true,"exit_code":0}]}))?;
        event_tx(db, "t", "VERIFICATION_REVISED", json!({"verification":[["cargo","test"], pinned(OLD), pinned(NEW)],"dropped_failed":[]}))?;
        event_tx(db, "t", "VERIFICATION_FAILED", json!({"unresolved_commands":[pinned(OLD)],"checks":[]}))?;
        Ok(())
    })
    .unwrap();
    let retired = s.write(|db| retire_superseded_tx(db, "t")).unwrap();
    assert_eq!(retired.len(), 1);
    assert_eq!(retired[0]["argv"], pinned(OLD));
    assert_eq!(verification(&s), json!([["cargo","test"], pinned(NEW)]));
    let recorded: i64 = s.read(|db| Ok(db.query_row(
        "SELECT count(*) FROM runtime_events WHERE type='VERIFICATION_REVISED' AND json_extract(payload_json,'$.superseded[0].reason') IS NOT NULL",
        [], |r| r.get(0))?)).unwrap();
    assert_eq!(recorded, 1);
    // Idempotent: nothing further to retire.
    assert!(s.write(|db| retire_superseded_tx(db, "t")).unwrap().is_empty());
}

/// Two pins on one path with different hashes contradict each other: keep the newest.
#[test]
fn contradictory_pins_keep_only_the_newest() {
    let (_p, s) = setup(json!([pinned(OLD), pinned(NEW)]));
    s.write(|db| {
        event_tx(db, "t", "PLAN_ACCEPTED", json!({"plan":{"verification":[pinned(OLD)]}}))?;
        event_tx(db, "t", "VERIFICATION_REVISED", json!({"verification":[pinned(OLD), pinned(NEW)],"dropped_failed":[]}))?;
        Ok(())
    })
    .unwrap();
    s.write(|db| retire_superseded_tx(db, "t")).unwrap();
    assert_eq!(verification(&s), json!([pinned(NEW)]));
}

/// A pinned check that never passed and contradicts nothing is still enforced.
#[test]
fn an_uncontradicted_pin_is_kept() {
    let (_p, s) = setup(json!([pinned(OLD), ["cargo","test"]]));
    assert!(s.write(|db| retire_superseded_tx(db, "t")).unwrap().is_empty());
    assert_eq!(verification(&s), json!([pinned(OLD), ["cargo","test"]]));
}

/// A sprint planning task has no plan of its own (its deliverable is the reviewed
/// sprint plan): the verification preflight finds nothing to retire instead of
/// failing on the absent plan with `storage_unavailable`.
#[test]
fn a_task_without_a_plan_has_nothing_to_retire() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='VERIFYING',plan_json=NULL WHERE task_id='t'", [])?;
        Ok(())
    })
    .unwrap();
    assert!(s.write(|db| retire_superseded_tx(db, "t")).unwrap().is_empty());
}
