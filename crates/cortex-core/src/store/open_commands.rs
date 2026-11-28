//! Derived index of starts without a later recorded outcome in the same task.
use crate::CoreResult;
use rusqlite::Connection;

const SCHEMA: &str = include_str!("open_commands.sql");
const OBJECTS: &str = "('runtime_command_outcomes_ref','runtime_open_commands',
    'runtime_open_commands_ref','runtime_open_commands_event','runtime_open_commands_dirty',
    'runtime_open_commands_start','runtime_open_commands_settle','runtime_open_commands_change',
    'runtime_open_commands_remove','runtime_open_commands_collision',
    'runtime_open_commands_update_collision')";

fn installed(db: &Connection) -> CoreResult<bool> {
    Ok(db.query_row(
        &format!("SELECT count(*)=11 FROM sqlite_schema WHERE name IN {OBJECTS}"),
        [],
        |row| row.get(0),
    )?)
}

pub(super) fn install(db: &Connection) -> CoreResult<()> {
    let complete = installed(db)?;
    db.execute_batch(SCHEMA)?;
    let scope = "task_id IN (SELECT task_id FROM runtime_open_commands_dirty)";
    if complete
        && !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_open_commands_dirty)",
            [],
            |row| row.get::<_, bool>(0),
        )?
    {
        return Ok(());
    }
    let (remove, restore) = if complete {
        (format!(" WHERE {scope}"), format!(" AND s.{scope}"))
    } else {
        (String::new(), String::new())
    };
    db.execute_batch(&format!(
        "DELETE FROM runtime_open_commands{remove};
         INSERT INTO runtime_open_commands
         SELECT s.task_id,s.id,json_extract(s.payload_json,'$.ref')
         FROM runtime_events s WHERE s.type='NATIVE_COMMAND_STARTED'{restore}
         AND NOT EXISTS(
            SELECT 1 FROM runtime_events f WHERE f.task_id=s.task_id AND f.id>s.id
             AND f.type IN ('NATIVE_COMMAND_FINISHED','NATIVE_COMMAND_RECOVERED')
             AND coalesce(json_extract(f.payload_json,'$.receipt.tool_call_ref'),json_extract(f.payload_json,'$.ref'))=json_extract(s.payload_json,'$.ref')
         );
         DELETE FROM runtime_open_commands_dirty;"
    ))?;
    Ok(())
}

pub(crate) fn available(db: &Connection, task: &str) -> CoreResult<bool> {
    if !installed(db)? {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM runtime_open_commands_dirty WHERE task_id=?)",
        [task],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn insert(db: &Connection, id: i64, task: &str, kind: &str, payload: &Value) {
        db.execute(
            "INSERT INTO runtime_events(id,task_id,type,actor,payload_json,created_at)
             VALUES (?,?,?,'runtime',?,'now')",
            (id, task, kind, payload.to_string()),
        )
        .unwrap();
    }

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(super::super::schema::SQL).unwrap();
        db.execute_batch(
            "PRAGMA foreign_keys=ON;
             INSERT INTO tasks VALUES ('a','x','now'),('b','x','now'),('c','x','now');
             INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES
                ('a','CREATED','a','now','now'),('b','CREATED','b','now','now'),
                ('c','CREATED','c','now','now');",
        )
        .unwrap();
        db
    }

    fn assert_matches_journal(db: &Connection) {
        let ids = |sql: &str, task: &str| {
            db.prepare(sql)
                .unwrap()
                .query_map([task], |row| row.get::<_, i64>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        for task in ["a", "b", "c"] {
            assert!(available(db, task).unwrap());
            let from_journal = ids(
                "SELECT s.id FROM runtime_events s WHERE s.task_id=? AND s.type='NATIVE_COMMAND_STARTED'
                 AND NOT EXISTS(SELECT 1 FROM runtime_events f WHERE f.task_id=s.task_id AND f.id>s.id
                    AND f.type IN ('NATIVE_COMMAND_FINISHED','NATIVE_COMMAND_RECOVERED')
                    AND coalesce(json_extract(f.payload_json,'$.receipt.tool_call_ref'),json_extract(f.payload_json,'$.ref'))=json_extract(s.payload_json,'$.ref'))
                 ORDER BY s.id",
                task,
            );
            let projected = ids(
                "SELECT start_id FROM runtime_open_commands WHERE task_id=? ORDER BY start_id",
                task,
            );
            assert_eq!(projected, from_journal, "{task}");
        }
    }

    #[test]
    fn backfill_and_random_insertion_match_ordered_journal() {
        for seed in [17_u64, 119, 503] {
            let mut db = database();
            let mut state = seed;
            let mut random = || {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state >> 32) as usize
            };
            let mut ids: Vec<i64> = (1..=600).collect();
            for i in (1..ids.len()).rev() {
                ids.swap(i, random() % (i + 1));
            }
            let references = [
                json!(null),
                json!(1),
                json!(1.0),
                json!("1"),
                json!("other"),
                json!(["x"]),
                json!("legacy"),
                json!("Case"),
                json!("case"),
                json!(""),
            ];
            for (i, id) in ids.into_iter().enumerate() {
                if i == 240 {
                    let tx = db.transaction().unwrap();
                    install(&tx).unwrap();
                    tx.commit().unwrap();
                    assert_matches_journal(&db);
                }
                let task = ["a", "b", "c"][random() % 3];
                let kind = match random() % 10 {
                    0..=5 => "NATIVE_COMMAND_STARTED",
                    6..=7 => "NATIVE_COMMAND_FINISHED",
                    8 => "NATIVE_COMMAND_RECOVERED",
                    _ => "UNRELATED",
                };
                let reference = &references[random() % references.len()];
                let receipt = &references[random() % references.len()];
                let payload = match random() % 4 {
                    0 => json!({"ref":reference, "receipt":{"tool_call_ref":receipt}}),
                    1 => json!({"ref":reference}),
                    2 => json!({"receipt":{"tool_call_ref":receipt}}),
                    _ => json!({}),
                };
                insert(&db, id, task, kind, &payload);
                if i >= 240 && i % 17 == 0 {
                    assert_matches_journal(&db);
                }
            }
            assert_matches_journal(&db);
        }
    }

    #[test]
    fn rollback_edits_conflicts_and_retention_keep_the_projection_safe() {
        let mut db = database();
        install(&db).unwrap();
        insert(&db, 10, "a", "NATIVE_COMMAND_STARTED", &json!({"ref":"x"}));
        insert(&db, 20, "b", "NATIVE_COMMAND_STARTED", &json!({"ref":"y"}));
        let settle_x = json!({"receipt":{"tool_call_ref":"x"}});
        let tx = db.transaction().unwrap();
        insert(&tx, 30, "a", "NATIVE_COMMAND_FINISHED", &settle_x);
        assert_matches_journal(&tx);
        tx.rollback().unwrap();
        assert_matches_journal(&db);

        insert(&db, 30, "a", "NATIVE_COMMAND_FINISHED", &settle_x);
        insert(
            &db,
            40,
            "b",
            "NATIVE_COMMAND_RECOVERED",
            &json!({"ref":"y"}),
        );
        // Same-id replacement suppresses SQLite's implicit delete trigger when
        // recursive triggers are off. Both original tasks must fall back.
        db.execute_batch(
            "PRAGMA recursive_triggers=OFF;
             INSERT OR REPLACE INTO runtime_events VALUES(30,'b','NATIVE_COMMAND_FINISHED','runtime','{}','now');
             UPDATE OR REPLACE runtime_events SET rowid=40 WHERE id=30;",
        ).unwrap();
        assert!(!available(&db, "a").unwrap());
        assert!(!available(&db, "b").unwrap());
        assert!(available(&db, "c").unwrap());
        install(&db).unwrap();
        assert_matches_journal(&db);

        insert(&db, 50, "a", "NATIVE_COMMAND_FINISHED", &settle_x);
        db.execute_batch("DELETE FROM runtime_events WHERE id=50;")
            .unwrap();
        assert!(!available(&db, "a").unwrap());
        install(&db).unwrap();
        assert_matches_journal(&db);
        db.execute_batch(
            "DELETE FROM runtime_events WHERE task_id='a';
                          DELETE FROM runtime_tasks WHERE task_id='a';",
        )
        .unwrap();
        assert_matches_journal(&db);
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM runtime_open_commands_dirty",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0,
        );
    }
}
