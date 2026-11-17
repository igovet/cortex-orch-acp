mod support;
use cortex_core::{
    channels::*,
    lifecycle::{FoundationGuards, TaskMachine},
    ownership, TaskState,
};
use support::*;

#[test]
fn external_calls_reserve_once_replay_exactly_and_fail_closed() {
    use cortex_core::external::{self, Reservation};
    use serde_json::json;
    let p = Project::new();
    let s = p.open();
    let owner = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLANNING'", [])?;
        Ok(())
    })
    .unwrap();
    let c = s
        .write(|db| issue_tx(db, "t", &owner, Principal::Coordinator))
        .unwrap();
    let digest = "a".repeat(64);
    assert!(s
        .write(|db| external::reserve_tx(db, &c, "fixture", &json!(1), &digest, "lookup", true))
        .is_err());
    s.write(|db| bind_session_tx(db, &c, "session")).unwrap();
    assert!(matches!(
        s.write(|db| external::reserve_tx(db, &c, "fixture", &json!(1), &digest, "lookup", true))
            .unwrap(),
        Reservation::Dispatch
    ));
    // A pending call is unknown, never sent again.
    assert!(matches!(
        s.write(|db| external::reserve_tx(db, &c, "fixture", &json!(1), &digest, "lookup", true))
            .unwrap(),
        Reservation::Unknown
    ));
    assert!(s
        .write(|db| external::reserve_tx(
            db,
            &c,
            "fixture",
            &json!(1),
            &"b".repeat(64),
            "lookup",
            true
        ))
        .is_err());
    assert!(s
        .write(|db| external::complete_tx(
            db,
            &c,
            "fixture",
            &json!(1),
            &json!({"jsonrpc":"2.0","id":2,"result":{}})
        ))
        .is_err());
    let reply = json!({"jsonrpc":"2.0","id":1,"result":{"fixture":true}});
    s.write(|db| external::complete_tx(db, &c, "fixture", &json!(1), &reply))
        .unwrap();
    match s
        .write(|db| external::reserve_tx(db, &c, "fixture", &json!(1), &digest, "lookup", true))
        .unwrap()
    {
        Reservation::Replay(saved) => assert_eq!(saved, reply),
        _ => panic!("missing replay"),
    }
    // String and numeric IDs and different servers cannot alias.
    for (server, id) in [("fixture", json!("1")), ("other", json!(1))] {
        assert!(matches!(
            s.write(|db| external::reserve_tx(db, &c, server, &id, &digest, "lookup", true))
                .unwrap(),
            Reservation::Dispatch
        ));
    }
    s.write(|db| revoke_tx(db, &c.channel_id)).unwrap();
    assert!(s
        .write(|db| external::reserve_tx(db, &c, "fixture", &json!(1), &digest, "lookup", true))
        .is_err());
    assert!(s
        .write(|db| external::complete_tx(db, &c, "other", &json!(1), &reply))
        .is_err());
}

#[test]
fn channel_secret_binding_expiry_epoch_and_revocation_are_rechecked() {
    let p = Project::new();
    let s = p.open();
    let owner = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    TaskMachine::new(&s)
        .transition(
            "t",
            &owner,
            "plan",
            TaskState::Planning,
            None,
            &FoundationGuards,
        )
        .unwrap();
    let c = s
        .write(|db| issue_tx(db, "t", &owner, Principal::Coordinator))
        .unwrap();
    s.read(|db| {
        assert!(authenticate(db, &c.channel_id, "not-the-secret", false).is_err());
        assert!(authenticate(db, &c.channel_id, c.secret(), true).is_err());
        assert_eq!(
            authenticate(db, &c.channel_id, c.secret(), false)?.role,
            "coordinator"
        );
        Ok(())
    })
    .unwrap();
    assert!(s
        .write(|db| issue_tx(db, "t", &owner, Principal::Coordinator))
        .is_err());
    s.write(|db| bind_session_tx(db, &c, "engine-session"))
        .unwrap();
    assert!(s
        .write(|db| bind_session_tx(db, &c, "other-session"))
        .is_err());
    s.read(|db| {
        assert_eq!(
            authenticate(db, &c.channel_id, c.secret(), true)?
                .session
                .as_deref(),
            Some("engine-session")
        );
        Ok(())
    })
    .unwrap();
    s.write(|db| {
        db.execute(
            "UPDATE trusted_channels SET expires_at='2000-01-01T00:00:00+00:00'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(s
        .read(|db| authenticate(db, &c.channel_id, c.secret(), true))
        .is_err());
    s.write(|db| {
        db.execute("UPDATE trusted_channels SET expires_at=NULL", [])?;
        ownership::acquire_tx(db, "t", "new", Some(owner.epoch))?;
        Ok(())
    })
    .unwrap();
    assert!(s
        .read(|db| authenticate(db, &c.channel_id, c.secret(), true))
        .is_err());
}

#[test]
fn worker_binding_requires_current_running_assignment_and_matching_stage() {
    let p = Project::new();
    let s = p.open();
    let o = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    assert!(s
        .write(|db| issue_tx(
            db,
            "t",
            &o,
            Principal::Worker {
                assignment: "missing"
            }
        ))
        .is_err());
    s.write(|db|{
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)",[])?;
        Ok(())
    }).unwrap();
    let c = s
        .write(|db| issue_tx(db, "t", &o, Principal::Worker { assignment: "a" }))
        .unwrap();
    s.write(|db| bind_session_tx(db, &c, "session")).unwrap();
    assert!(s
        .write(|db| issue_tx(db, "t", &o, Principal::Worker { assignment: "a" }))
        .is_err());
    s.read(|db| {
        let i = authenticate(db, &c.channel_id, c.secret(), true)?;
        assert_eq!(i.profile.as_deref(), Some("worker-backend-dev"));
        Ok(())
    })
    .unwrap();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='REVIEWING' WHERE task_id='t'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(s
        .read(|db| authenticate(db, &c.channel_id, c.secret(), true))
        .is_err());
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='t'",
            [],
        )?;
        revoke_tx(db, &c.channel_id)
    })
    .unwrap();
    assert!(s
        .read(|db| authenticate(db, &c.channel_id, c.secret(), true))
        .is_err());
}

#[test]
fn event_failure_rolls_back_channel_provisioning_with_its_runtime_transaction() {
    let p = Project::new();
    let s = p.open();
    let o = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLANNING'", [])?;
        Ok(())
    })
    .unwrap();
    let result = s.write(|db| {
        issue_tx(db, "t", &o, Principal::Coordinator)?;
        Err::<(), _>(cortex_core::CoreError::Refused("injected"))
    });
    assert!(result.is_err());
    assert_eq!(count(&s, "trusted_channels"), 0);
    assert_eq!(count(&s, "delegation_provisioning"), 0);
}
