mod support;

use cortex_core::{
    lifecycle::TaskMachine,
    ports::ProcessObservation,
    scheduler::{
        claim_ready,
        policy::{claim_obstacles_tx, write_conflict_tx, NativePolicy},
    },
    store::{owner_file_leases::*, schema},
    workspace::{create_draft, start_native},
    CoreError, CoreResult, OpenMode, Store,
};
use serde_json::json;
use std::{fs, os::unix::fs::symlink};
use support::{count, Project};

fn refused<T>(result: CoreResult<T>, code: &str) {
    assert_eq!(result.err().map(|e| e.code()), Some(code));
}

#[test]
fn grant_renew_expire_release_and_rotate_epoch_without_exposing_secrets() {
    let p = Project::new();
    let s = p.open();
    let checkout = Checkout::identify(&p.0).unwrap();
    let path = RelativePath::new("new.txt").unwrap();
    let epoch = EditorEpoch::new();
    let owner = Editor {
        epoch: &epoch,
        session: "local-1",
    };
    let other = Editor {
        session: "local-2",
        ..owner
    };
    refused(
        s.write(|db| acquire_tx(db, owner, &checkout, &path)),
        "edit_lease_lost",
    );
    s.write(|db| activate_editor_tx(db, &epoch)).unwrap();
    assert_eq!(
        s.read(|db| status(db, owner, &checkout, &path)).unwrap(),
        Status::Available
    );
    let token = s
        .write(|db| acquire_tx(db, owner, &checkout, &path))
        .unwrap();
    refused(
        s.write(|db| acquire_tx(db, other, &checkout, &path)),
        "owner_file_lease_held",
    );
    refused(
        s.write(|db| validate_save_tx(db, other, &checkout, &path, token.expose())),
        "edit_lease_lost",
    );
    assert!(matches!(
        s.read(|db| status(db, other, &checkout, &path)).unwrap(),
        Status::OtherEditor { .. }
    ));
    let before = s
        .write(|db| {
            let (hash, generation): (String, i64) = db.query_row(
                "SELECT token_hash,acquired_generation FROM code_owner_file_leases",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            // Avoid printing either value, even on failure.
            assert!(hash != token.expose() && hash.len() == 64);
            assert_eq!(generation, 1);
            db.execute(
                "UPDATE code_owner_file_leases SET expires_at=expires_at-30000",
                [],
            )?;
            Ok(
                db.query_row("SELECT expires_at FROM code_owner_file_leases", [], |r| {
                    r.get::<_, i64>(0)
                })?,
            )
        })
        .unwrap();
    s.write(|db| renew_tx(db, owner, &checkout, &path, token.expose()))
        .unwrap();
    let after = s
        .read(|db| {
            Ok(
                db.query_row("SELECT expires_at FROM code_owner_file_leases", [], |r| {
                    r.get::<_, i64>(0)
                })?,
            )
        })
        .unwrap();
    assert!(after >= before + 30_000);
    s.write(|db| validate_save_tx(db, owner, &checkout, &path, token.expose()))
        .unwrap();

    // Model the server deadline passing without a wall-clock sleep or client time.
    s.write(|db| {
        db.execute("UPDATE code_owner_file_leases SET expires_at=0", [])?;
        Ok(())
    })
    .unwrap();
    refused(
        s.write(|db| renew_tx(db, owner, &checkout, &path, token.expose())),
        "edit_lease_lost",
    );
    refused(
        s.write(|db| validate_save_tx(db, owner, &checkout, &path, token.expose())),
        "edit_lease_lost",
    );
    let token2 = s
        .write(|db| acquire_tx(db, other, &checkout, &path))
        .unwrap();
    s.write(|db| release_tx(db, other, &checkout, &path, token2.expose()))
        .unwrap();
    refused(
        s.write(|db| validate_save_tx(db, other, &checkout, &path, token2.expose())),
        "edit_lease_lost",
    );
    let token3 = s
        .write(|db| acquire_tx(db, other, &checkout, &path))
        .unwrap();
    drop(s);
    let s = p.open();
    let replacement = EditorEpoch::new();
    let new_owner = Editor {
        epoch: &replacement,
        ..owner
    };
    s.write(|db| activate_editor_tx(db, &replacement)).unwrap();
    refused(
        s.write(|db| validate_save_tx(db, other, &checkout, &path, token3.expose())),
        "edit_lease_lost",
    );
    refused(
        s.write(|db| acquire_tx(db, owner, &checkout, &path)),
        "edit_lease_lost",
    );
    s.write(|db| acquire_tx(db, new_owner, &checkout, &path))
        .unwrap();
    assert_eq!(s.write(|db| revoke_session_tx(db, new_owner)).unwrap(), 1);
    assert_eq!(
        s.read(|db| status(db, new_owner, &checkout, &path))
            .unwrap(),
        Status::Available
    );
}

#[test]
fn existing_grant_beats_new_authority_and_ambiguous_writers_do_not_expire() {
    let p = Project::new();
    let s = p.open();
    fs::create_dir(p.0.join("src")).unwrap();
    let root = Checkout::identify(&p.0).unwrap();
    let path = RelativePath::new("src/held").unwrap();
    let sibling = RelativePath::new("src/other").unwrap();
    let parent = RelativePath::new("src").unwrap();
    let epoch = EditorEpoch::new();
    let owner = Editor {
        epoch: &epoch,
        session: "window",
    };
    s.write(|db| activate_editor_tx(db, &epoch)).unwrap();
    s.write(|db| reserve_writer_tx(db, "first", &root, Coverage::Broad))
        .unwrap();
    refused(
        s.write(|db| acquire_tx(db, owner, &root, &path)),
        "checkout_busy",
    );
    assert!(s
        .write(|db| settle_writer_tx(db, "first", ProcessObservation::Unknown))
        .unwrap());
    drop(s);
    let s = p.open();
    let new_epoch = EditorEpoch::new();
    let owner = Editor {
        epoch: &new_epoch,
        ..owner
    };
    s.write(|db| activate_editor_tx(db, &new_epoch)).unwrap();
    refused(
        s.write(|db| acquire_tx(db, owner, &root, &path)),
        "writer_unconfirmed",
    );
    refused(
        s.write(|db| reserve_writer_tx(db, "first", &root, Coverage::Broad)),
        "writer_unconfirmed",
    );
    assert!(!s
        .write(|db| settle_writer_tx(db, "first", ProcessObservation::Alive))
        .unwrap());
    assert!(s
        .write(|db| settle_writer_tx(db, "first", ProcessObservation::ConfirmedExited))
        .unwrap());
    let token = s.write(|db| acquire_tx(db, owner, &root, &path)).unwrap();
    refused(
        s.write(|db| reserve_writer_tx(db, "second", &root, Coverage::Broad)),
        "owner_file_lease_held",
    );
    assert_eq!(count(&s, "code_writer_reservations"), 1);
    refused(
        s.write(|db| check_worker_effect_tx(db, &root, Coverage::FileConfined(&[parent]))),
        "owner_file_lease_held",
    );
    s.write(|db| {
        reserve_writer_tx(
            db,
            "confined",
            &root,
            Coverage::FileConfined(std::slice::from_ref(&sibling)),
        )
    })
    .unwrap();
    refused(
        s.write(|db| acquire_tx(db, owner, &root, &sibling)),
        "task_file_lease_held",
    );
    s.write(|db| release_tx(db, owner, &root, &path, token.expose()))
        .unwrap();
    refused(
        s.write(|db| reserve_writer_tx(db, "first", &root, Coverage::Broad)),
        "writer_unconfirmed",
    );
    s.write(|db| reserve_writer_tx(db, "second", &root, Coverage::Broad))
        .unwrap();
}

#[test]
fn independently_opened_stores_allow_only_one_first_writer() {
    let p = Project::new();
    let editor_store = p.open();
    let writer_store = Store::open(p.key(), OpenMode::RuntimeValidate).unwrap();
    let checkout = Checkout::identify(&p.0).unwrap();
    let path = RelativePath::new("held.txt").unwrap();
    let epoch = EditorEpoch::new();
    let editor = Editor {
        epoch: &epoch,
        session: "window",
    };
    editor_store
        .write(|db| activate_editor_tx(db, &epoch))
        .unwrap();
    let gate = std::sync::Barrier::new(2);
    std::thread::scope(|threads| {
        let owner = threads.spawn(|| {
            gate.wait();
            editor_store.write(|db| acquire_tx(db, editor, &checkout, &path))
        });
        gate.wait();
        let writer = writer_store
            .write(|db| reserve_writer_tx(db, "race-writer", &checkout, Coverage::Broad));
        let owner = owner.join().unwrap();
        if writer.is_ok() {
            refused(owner, "checkout_busy");
        } else {
            assert!(owner.is_ok());
            refused(writer, "owner_file_lease_held");
        }
    });
}

#[test]
fn unsafe_aliases_and_checkout_replacement_fail_closed() {
    let p = Project::new();
    let s = p.open();
    let root = Checkout::identify(&p.0).unwrap();
    let epoch = EditorEpoch::new();
    let owner = Editor {
        epoch: &epoch,
        session: "window",
    };
    s.write(|db| activate_editor_tx(db, &epoch)).unwrap();
    fs::create_dir(p.0.join("checkout")).unwrap();
    fs::write(p.0.join("checkout/file"), "base").unwrap();
    fs::hard_link(p.0.join("checkout/file"), p.0.join("checkout/hardlink")).unwrap();
    symlink(p.0.join("checkout"), p.0.join("link")).unwrap();
    for path in ["link/file", "checkout/file"] {
        let path = RelativePath::new(path).unwrap();
        refused(
            s.write(|db| acquire_tx(db, owner, &root, &path)),
            "unsafe_edit_path",
        );
    }
    for path in [
        ".git/index",
        "checkout/../file",
        "checkout//file",
        "/file",
        ".cortex-dev/db",
    ] {
        refused(RelativePath::new(path), "unsafe_edit_path");
    }
    let nested = Checkout::identify(&p.0.join("checkout")).unwrap();
    let alias = Checkout::identify(&p.0.join("link")).unwrap();
    let path = RelativePath::new("new").unwrap();
    let token = s.write(|db| acquire_tx(db, owner, &nested, &path)).unwrap();
    refused(
        s.write(|db| acquire_tx(db, owner, &alias, &path)),
        "owner_file_lease_held",
    );
    refused(
        s.write(|db| reserve_writer_tx(db, "parent", &root, Coverage::Broad)),
        "owner_file_lease_held",
    );
    fs::rename(p.0.join("checkout"), p.0.join("moved")).unwrap();
    fs::create_dir(p.0.join("checkout")).unwrap();
    refused(
        s.write(|db| validate_save_tx(db, owner, &nested, &path, token.expose())),
        "writer_unconfirmed",
    );
    let replacement = Checkout::identify(&p.0.join("checkout")).unwrap();
    refused(
        s.write(|db| reserve_writer_tx(db, "replacement", &replacement, Coverage::Broad)),
        "writer_unconfirmed",
    );
}

#[test]
fn format12_optional_group_is_installed_only_by_writable_initialize() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s)
        .create("preserve", "goal", "owner")
        .unwrap();
    s.write(|db| {
        db.execute_batch("DROP TABLE code_writer_reservations; DROP TABLE code_owner_file_leases; DROP TABLE code_editor_epoch")?;
        Ok(())
    }).unwrap();
    drop(s);
    for mode in [
        OpenMode::InspectImmutable,
        OpenMode::LiveReadOnly,
        OpenMode::RuntimeValidate,
    ] {
        let s = Store::open(p.key(), mode).unwrap();
        s.read(|db| {
            assert_eq!(schema::validate_header(db, false)?, 12);
            assert!(!schema::tables(db)?.contains("code_owner_file_leases"));
            Ok(())
        })
        .unwrap();
    }
    let s = p.open();
    assert_eq!(count(&s, "tasks"), 1);
    assert_eq!(count(&s, "code_owner_file_leases"), 0);
    s.write(|db| {
        db.execute_batch("DROP TABLE code_editor_epoch")?;
        Ok(())
    })
    .unwrap();
    drop(s);
    refused(
        Store::open(p.key(), OpenMode::LiveReadOnly),
        "partial_storage_schema",
    );
}

#[test]
fn only_overlapping_writer_scope_waits_for_owner_lease() {
    let p = Project::new();
    let s = p.open();
    let o = TaskMachine::new(&s).create("t", "goal", "runtime").unwrap();
    let epoch = EditorEpoch::new();
    let owner = Editor {
        epoch: &epoch,
        session: "window",
    };
    let root = Checkout::identify(&p.0).unwrap();
    let file = RelativePath::new("owner.txt").unwrap();
    let token = s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
        for (node, role, path) in [
            ("writer", "worker-backend-dev", "owner.txt"),
            ("reader", "worker-explorer", "owner.txt"),
            ("disjoint", "worker-backend-dev", "other.txt"),
        ] {
            db.execute(
                "INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status)
                 VALUES ('t',1,?,?,'goal','[]',?,'execution','pending')",
                (node, role, json!([path]).to_string()),
            )?;
        }
        activate_editor_tx(db, &epoch)?;
        acquire_tx(db, owner, &root, &file)
    }).unwrap();
    let plan = json!({"tasks":[
        {"id":"writer","role":"worker-backend-dev","scope":["owner.txt"],"dependencies":[]},
        {"id":"reader","role":"worker-qa-engineer","read_only":true,"scope":["owner.txt"],"dependencies":[]}
    ]});
    assert_eq!(
        s.read(|db| cortex_core::scheduler::policy::plan_scope_warnings(db, "t", &plan)).unwrap(),
        vec![json!({"reason":"owner_file_lease_held","node":"writer","paths":["owner.txt"]})]
    );
    assert_eq!(
        claim_ready(&s, "t", &o, "c", &NativePolicy::default())
            .unwrap()
            .len(),
        2
    );
    s.write(|db| {
        let conflict = write_conflict_tx(db, "t")?.unwrap();
        assert_eq!(conflict["node"], "writer");
        assert_eq!(conflict["reason"], "owner_file_lease_held");
        assert_eq!(conflict["paths"], json!(["owner.txt"]));
        assert!(conflict["retry_after_ms"]
            .as_u64()
            .is_some_and(|ms| ms <= 60_000));
        assert_eq!(
            claim_obstacles_tx(db, "t", 0.0)?[0]["reason"],
            "owner_file_lease_held"
        );
        release_tx(db, owner, &root, &file, token.expose())
    })
    .unwrap();
    assert_eq!(
        claim_ready(&s, "t", &o, "c", &NativePolicy::default())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn native_start_does_not_leave_a_broad_hold_and_active_writer_names_task() {
    let p = Project::new();
    let s = p.open();
    let root = Checkout::identify(&p.0).unwrap();
    let epoch = EditorEpoch::new();
    let owner = Editor {
        epoch: &epoch,
        session: "window",
    };
    let file = RelativePath::new("owner.txt").unwrap();
    let token = s
        .write(|db| {
            db.execute(
                "INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)",
                [root.path().to_str()],
            )?;
            activate_editor_tx(db, &epoch)?;
            acquire_tx(db, owner, &root, &file)
        })
        .unwrap();
    let create = json!({
        "protocol":{"major":1,"minor":8},"requestId":"create","projectId":"p","actorId":"owner",
        "operation":"tasks.create","target":"p","expectedVersion":"0","ownerEpoch":null,
        "payload":{"description":"Goal","checkout":{"kind":"main"}}
    });
    let draft = create_draft(&s, "p", "owner", &create).unwrap().receipt;
    let start = json!({
        "protocol":{"major":1,"minor":8},"requestId":"start","projectId":"p","actorId":"owner",
        "operation":"tasks.start","target":draft["target"],"expectedVersion":draft["entityVersion"],
        "ownerEpoch":null,"payload":{}
    });
    let admitted = start_native(&s, "p", "owner", &start).unwrap();
    assert_eq!(admitted["fresh"], true);
    assert_eq!(
        start_native(&s, "p", "owner", &start).unwrap()["fresh"],
        false
    );
    assert_eq!(count(&s, "code_writer_reservations"), 0);
    let task = s.write(|db| {
        let task: String = db.query_row("SELECT task_id FROM runtime_tasks", [], |r| r.get(0))?;
        release_tx(db, owner, &root, &file, token.expose())?;
        db.execute(
            "INSERT INTO code_writer_reservations VALUES (1,'start:'||?,?,
             (SELECT device FROM code_owner_file_leases),(SELECT inode FROM code_owner_file_leases),NULL,'reserved')",
            (&task, root.path().to_str()),
        )?;
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id=?", [&task])?;
        db.execute(
            "INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status)
             VALUES (?,1,'writer','worker-backend-dev','goal','[]','[\"owner.txt\"]','execution','running')",
            [&task],
        )?;
        Ok(task)
    }).unwrap();
    let err = s
        .write(|db| acquire_tx(db, owner, &root, &file))
        .err()
        .unwrap();
    assert!(matches!(err, CoreError::TaskFileLeaseHeld { task: holder } if holder == task));
    let sibling = RelativePath::new("other.txt").unwrap();
    s.write(|db| acquire_tx(db, owner, &root, &sibling))
        .unwrap();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id=?",
            [&task],
        )?;
        acquire_tx(db, owner, &root, &file)
    })
    .unwrap();
}

#[test]
fn old_boot_or_unqualified_epoch_does_not_hold_a_file_before_service_activation() {
    let p = Project::new();
    let s = p.open();
    let root = Checkout::identify(&p.0).unwrap();
    let epoch = EditorEpoch::new();
    let owner = Editor {
        epoch: &epoch,
        session: "window",
    };
    let file = RelativePath::new("src/held").unwrap();
    fs::create_dir(p.0.join("src")).unwrap();
    for previous in [
        "old-unqualified-epoch",
        "00000000000000000000000000000000:previous",
    ] {
        let token = s
            .write(|db| {
                activate_editor_tx(db, &epoch)?;
                acquire_tx(db, owner, &root, &file)
            })
            .unwrap();
        assert!(matches!(
            s.read(|db| status_with_token(db, owner, &root, &file, token.expose()))
                .unwrap(),
            Status::Yours { .. }
        ));
        for destination in ["src/held", "src", "src/held/child"] {
            let paths = [
                RelativePath::new("src/elsewhere").unwrap(),
                RelativePath::new(destination).unwrap(),
            ];
            let err = s
                .write(|db| validate_worker_paths_tx(db, &root, &paths))
                .unwrap_err();
            assert!(matches!(err, CoreError::OwnerFileLeaseHeld { path } if path == file));
        }
        s.write(|db| {
            db.execute("UPDATE code_editor_epoch SET epoch=?", [previous])?;
            db.execute(
                "UPDATE code_owner_file_leases SET epoch=?, expires_at=?",
                (previous, i64::MAX),
            )?;
            validate_worker_paths_tx(db, &root, std::slice::from_ref(&file))
        })
        .unwrap();
        refused(
            s.write(|db| renew_tx(db, owner, &root, &file, token.expose())),
            "edit_lease_lost",
        );
        refused(
            s.read(|db| status_with_token(db, owner, &root, &file, token.expose())),
            "edit_lease_lost",
        );
    }
}
