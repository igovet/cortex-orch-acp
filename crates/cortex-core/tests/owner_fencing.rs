mod support;
use cortex_core::{
    lifecycle::TaskMachine,
    ownership::{self, ProjectOwner},
};
use support::*;
#[test]
fn project_owner_uses_separate_lifetime_lock() {
    let p = Project::new();
    let s = p.open();
    let owner = ProjectOwner::acquire(&p.key()).unwrap();
    assert!(ProjectOwner::acquire(&p.key()).is_err());
    // Access remains shared and useful while the Core owns the project.
    s.read(|_| Ok(())).unwrap();
    drop(owner);
    assert!(ProjectOwner::acquire(&p.key()).is_ok());
}
/// Helper child: only runs when re-executed by the test below; it holds the
/// scheduler owner of the directory in `CORTEX_FENCING_CHILD_DIR` until stdin gets a line.
#[test]
fn scheduler_owner_holder_child() {
    let Some(dir) = std::env::var_os("CORTEX_FENCING_CHILD_DIR") else {
        return;
    };
    use cortex_core::ownership::SchedulerOwner;
    use std::io::{BufRead, Write};
    let key =
        cortex_core::ProjectKey::new(std::path::Path::new(&dir), cortex_core::StoreChannel::Dev)
            .unwrap();
    let _owner = SchedulerOwner::acquire(&key).unwrap();
    println!("holder-ready");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
}
#[test]
fn scheduler_owner_excludes_another_process_and_keeps_reads_available() {
    use cortex_core::{ownership::SchedulerOwner, store::executor::StoreExecutor};
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    let p = Project::new();
    let s = p.open();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "scheduler_owner_holder_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CORTEX_FENCING_CHILD_DIR", &p.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    while !line.contains("holder-ready") {
        line.clear();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "child exited before ready"
        );
    }
    assert_eq!(
        StoreExecutor::open(p.key()).err().unwrap().code(),
        "scheduler_owner_busy"
    );
    s.read(|_| Ok(())).unwrap();
    child.stdin.take().unwrap().write_all(b"stop\n").unwrap();
    assert!(child.wait().unwrap().success());
    let executor = StoreExecutor::open(p.key()).unwrap();
    assert_eq!(
        SchedulerOwner::acquire(&p.key()).err().unwrap().code(),
        "scheduler_owner_busy"
    );
    drop(executor);
    assert!(SchedulerOwner::acquire(&p.key()).is_ok());
}

#[test]
fn concurrent_epoch_cas_has_exactly_one_winner() {
    let p = Project::new();
    let s = std::sync::Arc::new(p.open());
    let owner = TaskMachine::new(&s).create("t", "goal", "initial").unwrap();
    let threads: Vec<_> = (0..8)
        .map(|n| {
            let s = s.clone();
            std::thread::spawn(move || {
                s.write(|db| {
                    ownership::acquire_tx(db, "t", &format!("owner-{n}"), Some(owner.epoch))
                })
                .is_ok()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter(|ok| *ok)
            .count(),
        1
    );
}

#[test]
fn shared_writer_hands_the_lease_over_only_while_idle() {
    use cortex_core::{ownership::SchedulerOwner, store::executor::StoreExecutor};
    let p = Project::new();
    let shared = StoreExecutor::open_shared(p.key()).unwrap();
    // Held while open and recently used.
    assert_eq!(
        SchedulerOwner::acquire(&p.key()).err().unwrap().code(),
        "scheduler_owner_busy"
    );
    // An unfinished task keeps the lease however long the writer is idle.
    shared
        .write(|db| {
            db.execute("INSERT INTO tasks VALUES ('t','/p',?)", [cortex_core::now()])?;
            db.execute(
                "INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('t','PLANNING','g',?,?)",
                (cortex_core::now(), cortex_core::now()),
            )?;
            Ok(())
        })
        .unwrap()
        .blocking_recv()
        .unwrap()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert_eq!(
        SchedulerOwner::acquire(&p.key()).err().unwrap().code(),
        "scheduler_owner_busy"
    );
    // Once every task is finished the idle writer releases it ...
    shared
        .write(|db| {
            db.execute("UPDATE runtime_tasks SET state='COMPLETED'", [])?;
            Ok(())
        })
        .unwrap()
        .blocking_recv()
        .unwrap()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500));
    let other = SchedulerOwner::acquire(&p.key()).expect("idle shared writer released the lease");
    // ... a job is then refused, never run beside the other holder ...
    let refused = shared.write(|_| Ok(())).unwrap().blocking_recv().unwrap();
    assert_eq!(refused.err().unwrap().code(), "scheduler_owner_busy");
    // ... and retakes the lease once the other holder is gone.
    drop(other);
    shared
        .write(|_| Ok(()))
        .unwrap()
        .blocking_recv()
        .unwrap()
        .unwrap();
    assert_eq!(
        SchedulerOwner::acquire(&p.key()).err().unwrap().code(),
        "scheduler_owner_busy"
    );
}

/// A live process never releases the shared lease with an unfinished task, so an
/// unfinished task found on retake was left by a process that is gone: the idle
/// process keeps the lease, runs its job and reports the orphan for recovery.
#[test]
fn idle_shared_writer_adopts_a_task_left_by_a_vanished_peer() {
    use cortex_core::store::executor::StoreExecutor;
    let p = Project::new();
    let a = StoreExecutor::open_shared(p.key()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    {
        // Peer B takes the released lease, starts a task and vanishes mid-task.
        let b = StoreExecutor::open_shared(p.key()).unwrap();
        b.write(|db| {
            db.execute("INSERT INTO tasks VALUES ('orphan','/p',?)", [cortex_core::now()])?;
            db.execute(
                "INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('orphan','EXECUTING','g',?,?)",
                (cortex_core::now(), cortex_core::now()),
            )?;
            Ok(())
        })
        .unwrap()
        .blocking_recv()
        .unwrap()
        .unwrap();
        assert!(!b.take_orphaned());
    }
    assert!(!a.take_orphaned());
    a.write(|_| Ok(()))
        .unwrap()
        .blocking_recv()
        .unwrap()
        .expect("the job runs; it is not refused");
    assert!(
        a.take_orphaned(),
        "the orphaned task is reported for recovery once"
    );
    assert!(!a.take_orphaned());
}
