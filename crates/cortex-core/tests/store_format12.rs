mod support;
use cortex_core::{OpenMode, ProjectKey, Store, StoreChannel};
use rusqlite::Connection;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    os::unix::fs::{symlink, PermissionsExt},
};
use support::*;
#[test]
fn schema_matches_frozen_python_base_and_rust_additions() {
    let p = Project::new();
    let s = p.open();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/python_format12.json")).unwrap();
    s.read(|db| {
        assert_eq!(db.query_row("PRAGMA application_id",[],|r|r.get::<_,i64>(0))?,fixture["application_id"].as_i64().unwrap());
        assert_eq!(db.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0))?,12);
        let actual=db.prepare("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'")?
            .query_map([],|r|Ok((r.get::<_,String>(1)?,(r.get::<_,String>(0)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))))?
            .collect::<Result<BTreeMap<_,_>,_>>()?;
        // Keep the historical base fixture byte-for-byte. Assert the entire
        // current schema by supplementing it with independently frozen DDL
        // for the additive Rust groups, not by filtering those groups out.
        let additions: Vec<Value> =
            serde_json::from_str(include_str!("fixtures/rust_additive_format12.json")).unwrap();
        let mut expected = BTreeMap::new();
        for o in fixture["objects"].as_array().unwrap().iter().chain(&additions) {
            let name = o["name"].as_str().unwrap().to_owned();
            let definition = (
                o["type"].as_str().unwrap().to_owned(),
                o["tbl_name"].as_str().unwrap().to_owned(),
                o["sql"].as_str().unwrap().to_owned(),
            );
            assert!(expected.insert(name, definition).is_none());
        }
        assert_eq!(actual.keys().collect::<Vec<_>>(), expected.keys().collect::<Vec<_>>());
        for (name, definition) in expected {
            assert_eq!(actual[&name], definition, "{name}");
        }
        for (table,columns) in fixture["columns"].as_object().unwrap() {
            let actual=db.prepare(&format!("PRAGMA table_info({table})"))?.query_map([],|r|Ok(serde_json::json!([
                r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,
                r.get::<_,Option<String>>(4)?,r.get::<_,i64>(5)?
            ])))?.collect::<Result<Vec<_>,_>>()?;
            assert_eq!(serde_json::json!(actual),*columns,"{table}");
        }
        assert_eq!(db.query_row("PRAGMA journal_mode",[],|r|r.get::<_,String>(0))?,"wal");
        assert_eq!(db.query_row("PRAGMA synchronous",[],|r|r.get::<_,i64>(0))?,2);
        assert_eq!(db.query_row("PRAGMA foreign_keys",[],|r|r.get::<_,i64>(0))?,1);
        Ok(())
    }).unwrap();
}
#[test]
fn rollback_on_error_and_foreign_keys_enforced() {
    let p = Project::new();
    let s = p.open();
    let result = s.write(|db| {
        db.execute("INSERT INTO tasks VALUES ('task','x','now')", [])?;
        Err::<(), _>(cortex_core::CoreError::Refused("injected"))
    });
    assert!(result.is_err());
    assert_eq!(count(&s, "tasks"), 0);
    assert!(s.write(|db|{db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('missing','CREATED','x','x','x')",[])?;Ok(())}).is_err());
}
#[test]
fn immutable_inspection_is_byte_identical_and_refuses_mutation() {
    let p = Project::new();
    drop(p.open());
    let before = std::fs::read(p.key().database()).unwrap();
    let entries = std::fs::read_dir(p.key().directory()).unwrap().count();
    let s = Store::open(p.key(), OpenMode::InspectImmutable).unwrap();
    assert!(s.write(|_| Ok(())).is_err());
    assert!(s
        .read(|db| {
            db.execute("CREATE TABLE forbidden(x)", [])?;
            Ok(())
        })
        .is_err());
    drop(s);
    assert_eq!(before, std::fs::read(p.key().database()).unwrap());
    assert_eq!(
        entries,
        std::fs::read_dir(p.key().directory()).unwrap().count()
    );
}
#[test]
fn inspection_never_creates_missing_store_or_access_lock() {
    let p = Project::new();
    assert!(Store::open(p.key(), OpenMode::InspectImmutable).is_err());
    assert!(!p.key().directory().exists());
    drop(p.open());
    std::fs::remove_file(p.key().directory().join(".access.lock")).unwrap();
    assert!(Store::open(p.key(), OpenMode::InspectImmutable).is_err());
    assert!(!p.key().directory().join(".access.lock").exists());
}
#[test]
fn live_wal_is_read_by_runtime_but_never_ignored_by_inspection() {
    let p = Project::new();
    let s = p.open();
    s.write(|db| {
        db.execute("INSERT INTO tasks VALUES ('task',?,'now')", [p.0.to_str()])?;
        Ok(())
    })
    .unwrap();
    let r = Store::open(p.key(), OpenMode::RuntimeValidate).unwrap();
    assert_eq!(count(&r, "tasks"), 1);
    let err = Store::open(p.key(), OpenMode::InspectImmutable)
        .err()
        .unwrap();
    assert_eq!(err.code(), "storage_inspection_requires_checkpoint");
}
#[test]
fn format10_unknown_and_foreign_database_fail_without_rewrite() {
    for (app, version) in [(1129467218, 10), (1129467218, 13), (99, 12)] {
        let p = Project::new();
        drop(p.open());
        let db = Connection::open(p.key().database()).unwrap();
        db.pragma_update(None, "user_version", version).unwrap();
        db.pragma_update(None, "application_id", app).unwrap();
        drop(db);
        let before = std::fs::read(p.key().database()).unwrap();
        for mode in [
            OpenMode::Initialize,
            OpenMode::RuntimeValidate,
            OpenMode::InspectImmutable,
        ] {
            assert_eq!(
                Store::open(p.key(), mode).err().unwrap().code(),
                "unsupported_storage"
            );
        }
        assert_eq!(before, std::fs::read(p.key().database()).unwrap());
    }
}
#[test]
fn optional_groups_absent_stay_absent_partial_fails_then_initialization_repairs() {
    let p = Project::new();
    drop(p.open());
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch("DROP TABLE workspace_task_options; DROP TABLE workspace_starts; DROP TABLE workspace_requests;
        DROP TABLE workspace_outbox; DROP TABLE workspace_outbox_state; DROP TABLE workspace_executions;
        DROP TABLE workspace_tasks; DROP TABLE workspace_projects;").unwrap();
    drop(db);
    let s = Store::open(p.key(), OpenMode::InspectImmutable).unwrap();
    s.read(|db| {
        assert!(!cortex_core::store::schema::tables(db)?.contains("workspace_tasks"));
        Ok(())
    })
    .unwrap();
    drop(s);
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/python_format12.json")).unwrap();
    let sql = fixture["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["name"] == "workspace_projects")
        .unwrap()["sql"]
        .as_str()
        .unwrap();
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch(sql).unwrap();
    drop(db);
    assert_eq!(
        Store::open(p.key(), OpenMode::InspectImmutable)
            .err()
            .unwrap()
            .code(),
        "partial_storage_schema"
    );
    let repaired = p.open();
    repaired
        .read(|db| {
            assert!(cortex_core::store::schema::tables(db)?.contains("workspace_tasks"));
            Ok(())
        })
        .unwrap();
}
/// An existing format-12 store gains the conversation call index on its next writable open,
/// even with a malformed retained update (the partial index never evaluates it), and keeps
/// accepting one. Inspection never installs it.
#[test]
fn an_existing_store_gains_the_call_index_on_a_writable_open_only() {
    let p = Project::new();
    drop(p.open());
    let has = |db: &Connection| {
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='host_updates_by_call'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    };
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch(
        "DROP INDEX host_updates_by_call; PRAGMA foreign_keys=OFF;
         INSERT INTO host_updates VALUES ('s',0,'not json','now');
         INSERT INTO host_updates VALUES ('s',1,'{\"toolCallId\":\"c\"}','now');",
    )
    .unwrap();
    drop(db);
    // A store without the index is still accepted by every mode; only Initialize adds it.
    drop(Store::open(p.key(), OpenMode::InspectImmutable).unwrap());
    drop(Store::open(p.key(), OpenMode::LiveReadOnly).unwrap());
    drop(Store::open(p.key(), OpenMode::RuntimeValidate).unwrap());
    assert!(!has(&Connection::open(p.key().database()).unwrap()));
    let s = p.open();
    s.read(|db| {
        assert!(has(db));
        let calls: i64 = db.query_row(
            "SELECT count(*) FROM host_updates INDEXED BY host_updates_by_call WHERE session_id='s' AND json_valid(update_json) AND json_extract(update_json,'$.toolCallId')='c'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(calls, 1);
        Ok(())
    })
    .unwrap();
    drop(s);
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch(
        "PRAGMA foreign_keys=OFF; INSERT INTO host_updates VALUES ('s',2,'still not json','now');",
    )
    .unwrap();
}
/// An existing format-12 store gains the tool-call reference indexes on its next writable
/// open only, and the hot per-task lookups use them instead of parsing every payload.
#[test]
fn an_existing_store_gains_the_tool_call_indexes_on_a_writable_open_only() {
    let p = Project::new();
    drop(p.open());
    let count = |db: &Connection| {
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('runtime_events_by_tool_call_ref','runtime_events_finished_ref')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch("DROP INDEX runtime_events_by_tool_call_ref; DROP INDEX runtime_events_finished_ref;")
        .unwrap();
    drop(db);
    drop(Store::open(p.key(), OpenMode::InspectImmutable).unwrap());
    drop(Store::open(p.key(), OpenMode::LiveReadOnly).unwrap());
    assert_eq!(count(&Connection::open(p.key().database()).unwrap()), 0);
    let s = p.open();
    s.read(|db| {
        assert_eq!(count(db), 2);
        let plan = |sql: &str| -> String {
            let mut q = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            q.query_map([], |r| r.get::<_, String>(3))
                .unwrap()
                .map(|r| r.unwrap())
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let by_ref = plan("SELECT payload_json FROM runtime_events WHERE task_id='t' AND type='TOOL_COMPLETED' AND json_extract(payload_json,'$.tool_call_ref')='r' ORDER BY id DESC LIMIT 1");
        assert!(by_ref.contains("runtime_events_by_tool_call_ref"), "{by_ref}");
        let finished = plan("SELECT 1 FROM runtime_events f WHERE f.task_id='t' AND f.type='NATIVE_COMMAND_FINISHED' AND json_extract(f.payload_json,'$.receipt.tool_call_ref')='r'");
        assert!(finished.contains("runtime_events_finished_ref"), "{finished}");
        Ok(())
    })
    .unwrap();
}
#[test]
fn channels_never_read_or_import_the_other_store() {
    let p = Project::new();
    let dev = p.open();
    let release = ProjectKey::new(&p.0, StoreChannel::Release).unwrap();
    assert_ne!(release.database(), p.key().database());
    assert!(!release.directory().exists());
    assert!(Store::open(release.clone(), OpenMode::RuntimeValidate).is_err());
    assert!(!release.directory().exists());
    drop(dev);
}
#[test]
fn symlink_and_hardlink_database_and_nonprivate_directory_refused() {
    let p = Project::new();
    drop(p.open());
    let original = p.key().database();
    let saved = p.0.join("copy");
    std::fs::rename(&original, &saved).unwrap();
    symlink(&saved, &original).unwrap();
    assert!(Store::open(p.key(), OpenMode::Initialize).is_err());
    std::fs::remove_file(&original).unwrap();
    std::fs::hard_link(&saved, &original).unwrap();
    assert!(Store::open(p.key(), OpenMode::Initialize).is_err());
    std::fs::remove_file(&saved).unwrap();
    std::fs::set_permissions(p.key().directory(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Store::open(p.key(), OpenMode::RuntimeValidate).is_err());
}
#[test]
fn linked_worktree_shares_logical_channel_store() {
    let p = Project::new();
    let run = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .current_dir(&p.0)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap()
            .success())
    };
    run(&["init", "-q"]);
    run(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@invalid",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "fixture",
    ]);
    let linked = p.0.join("linked");
    run(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "fixture",
        linked.to_str().unwrap(),
    ]);
    assert_eq!(
        ProjectKey::new(&linked, StoreChannel::Dev).unwrap(),
        p.key()
    );
}
/// An existing format-12 store gains the narration index and planner statistics on its next
/// writable open, transactionally and idempotently; every other mode reads it unchanged. A worker's
/// narration lookup (its `{attempt}:` message identities in one session) then seeks that index
/// instead of walking the whole session.
#[test]
fn an_existing_store_gains_the_narration_index_and_statistics_on_a_writable_open_only() {
    let p = Project::new();
    drop(p.open());
    let state = |db: &Connection| {
        let index: i64 = db
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='host_transcript_by_message'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let stats: i64 = db
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='sqlite_stat1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        (index == 1, stats == 1)
    };
    let db = Connection::open(p.key().database()).unwrap();
    db.execute_batch(
        "DROP INDEX host_transcript_by_message; DROP TABLE IF EXISTS sqlite_stat1; PRAGMA foreign_keys=OFF;",
    )
    .unwrap();
    {
        let mut insert = db
            .prepare("INSERT INTO host_transcript(session_id,seq,kind,text,message_id,created_at) VALUES ('s',?,'agent_message','x',?,'now')")
            .unwrap();
        for seq in 0..2000i64 {
            insert
                .execute((seq, format!("xa_{}:msg_{seq}", seq % 50)))
                .unwrap();
        }
    }
    drop(db);
    assert_eq!(state(&Connection::open(p.key().database()).unwrap()), (false, false));
    drop(Store::open(p.key(), OpenMode::InspectImmutable).unwrap());
    drop(Store::open(p.key(), OpenMode::LiveReadOnly).unwrap());
    drop(Store::open(p.key(), OpenMode::RuntimeValidate).unwrap());
    assert_eq!(state(&Connection::open(p.key().database()).unwrap()), (false, false));
    let plan = |db: &Connection| {
        db.prepare(
            "EXPLAIN QUERY PLAN SELECT seq,message_id,text FROM host_transcript
             WHERE session_id='s' AND kind='agent_message' AND message_id>='xa_7:' AND message_id<'xa_7;'
             ORDER BY seq DESC LIMIT 4001",
        )
        .unwrap()
        .query_map([], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n")
    };
    for _ in 0..2 {
        let s = p.open();
        s.read(|db| {
            assert_eq!(state(db), (true, true));
            let plan = plan(db);
            assert!(plan.contains("host_transcript_by_message"), "{plan}");
            let found: i64 = db.query_row(
                "SELECT count(*) FROM host_transcript WHERE session_id='s' AND kind='agent_message'
                 AND message_id>='xa_7:' AND message_id<'xa_7;'",
                [],
                |r| r.get(0),
            )?;
            // The range holds exactly the identities with that prefix.
            let prefixed: i64 = db.query_row(
                "SELECT count(*) FROM host_transcript WHERE session_id='s' AND kind='agent_message'
                 AND substr(message_id,1,5)='xa_7:'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!((found, prefixed), (40, 40));
            Ok(())
        })
        .unwrap();
        drop(s);
    }
    // A store that has them is still read by every mode.
    drop(Store::open(p.key(), OpenMode::InspectImmutable).unwrap_or_else(|e| panic!("{}", e.code())));
    drop(Store::open(p.key(), OpenMode::LiveReadOnly).unwrap());
}
