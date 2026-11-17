mod support;
use cortex_core::{lifecycle::TaskMachine, ports::ProcessObservation, scheduler::*};
use support::*;
fn node(id: &str, status: &str, deps: &[&str]) -> Node {
    Node {
        id: id.into(),
        stage: "execution".into(),
        status: status.into(),
        dependencies: deps.iter().map(|s| (*s).into()).collect(),
    }
}
#[test]
fn dependency_selection_prioritizes_longest_chain_and_rejects_bad_graphs() {
    let nodes = [
        node("short", "pending", &[]),
        node("long", "pending", &[]),
        node("child", "pending", &["long"]),
        node("deep", "pending", &["child"]),
    ];
    assert_eq!(
        dependency_ready(&nodes, "execution").unwrap(),
        ["long", "short"]
    );
    assert!(dependency_ready(&[node("a", "pending", &["b"])], "execution").is_err());
    assert!(dependency_ready(
        &[node("a", "pending", &["b"]), node("b", "pending", &["a"])],
        "execution"
    )
    .is_err());
    assert_eq!(
        dependency_ready(
            &[node("a", "completed", &[]), node("b", "pending", &["a"])],
            "execution"
        )
        .unwrap(),
        ["b"]
    );
}
#[test]
fn ambiguous_capacity_survives_restart_and_only_confirmed_exit_releases() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "o").unwrap();
    let mut ids = Vec::new();
    for n in 0..4 {
        ids.push(
            s.write(|db| {
                reserve_tx(
                    db,
                    &Reservation {
                        task: "t",
                        connection: "c",
                        assignment: None,
                        attempt: &format!("attempt-{n}"),
                        kind: ReservationKind::Worker,
                        connection_limit: 4,
                    },
                )
            })
            .unwrap()
            .unwrap(),
        );
    }
    s.write(|db| {
        settle_tx(db, &ids[0], ProcessObservation::Unknown)?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = p.open();
    assert!(s
        .write(|db| reserve_tx(
            db,
            &Reservation {
                task: "t",
                connection: "other",
                assignment: None,
                attempt: "extra",
                kind: ReservationKind::Worker,
                connection_limit: 4
            }
        ))
        .unwrap()
        .is_none());
    assert!(s
        .write(|db| reserve_tx(
            db,
            &Reservation {
                task: "t",
                connection: "c",
                assignment: None,
                attempt: "coordinator",
                kind: ReservationKind::Coordinator,
                connection_limit: 1
            }
        ))
        .unwrap()
        .is_some());
    assert!(!s
        .write(|db| settle_tx(db, &ids[0], ProcessObservation::Alive))
        .unwrap());
    assert!(s
        .write(|db| settle_tx(db, &ids[0], ProcessObservation::ConfirmedExited))
        .unwrap());
    assert!(!s
        .write(|db| settle_tx(db, &ids[0], ProcessObservation::Unknown))
        .unwrap());
    assert!(s
        .write(|db| reserve_tx(
            db,
            &Reservation {
                task: "t",
                connection: "c",
                assignment: None,
                attempt: "replacement",
                kind: ReservationKind::Worker,
                connection_limit: 4
            }
        ))
        .unwrap()
        .is_some());
}
#[test]
fn coordinator_and_other_tasks_do_not_consume_this_tasks_worker_slots() {
    let p = Project::new();
    let s = p.open();
    for task in ["first", "second"] {
        TaskMachine::new(&s).create(task, "goal", "o").unwrap();
    }
    for task in ["first", "second"] {
        assert!(s
            .write(|db| reserve_tx(
                db,
                &Reservation {
                    task,
                    connection: "c",
                    assignment: None,
                    attempt: task,
                    kind: ReservationKind::Worker,
                    connection_limit: 1
                }
            ))
            .unwrap()
            .is_some());
    }
    assert!(s
        .write(|db| reserve_tx(
            db,
            &Reservation {
                task: "first",
                connection: "c",
                assignment: None,
                attempt: "busy",
                kind: ReservationKind::Worker,
                connection_limit: 1
            }
        ))
        .unwrap()
        .is_none());
}
