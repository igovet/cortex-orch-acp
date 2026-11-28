//! Client-owned access authority. Missing/legacy state is always Guarded.
//! Revisions use the existing append-only runtime event journal, not display history.
use crate::{lifecycle, CoreError, CoreResult};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub const MODES: [&str; 3] = ["development", "guarded", "auto"];
pub fn normalize(mode: &str) -> &str {
    if MODES.contains(&mode) {
        mode
    } else {
        "guarded"
    }
}
// Find the last id for each type independently so SQLite can seek to each
// maximum using runtime_events_type, then read only the winning payload.
const CURRENT_SQL: &str =
    "SELECT json_extract(payload_json,'$.mode') FROM runtime_events WHERE id=(
        SELECT max(id) FROM (
            SELECT max(id) AS id FROM runtime_events WHERE type='NATIVE_ACCESS_INITIALIZED' AND task_id=?1
            UNION ALL
            SELECT max(id) FROM runtime_events WHERE type='NATIVE_ACCESS_SWITCHED' AND task_id=?1
        )
    )";
pub fn current(db: &Connection, task: &str) -> CoreResult<String> {
    let raw: Option<String> = db.query_row(CURRENT_SQL, [task], |r| r.get(0)).optional()?;
    Ok(normalize(raw.as_deref().unwrap_or("")).into())
}
pub fn session(db: &Connection, sid: &str) -> CoreResult<String> {
    let row: Option<(String, Option<String>, Option<String>)> = db.query_row(
        "SELECT mode,selected_mode,selection_origin FROM acp_session_access_defaults WHERE session_id=?",
        [sid], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    Ok(match row {
        Some((_, Some(selected), Some(origin)))
            if matches!(
                origin.as_str(),
                "acp_config_option" | "acp_set_mode" | "acp_pre_task_selection"
            ) =>
        {
            normalize(&selected).into()
        }
        Some((default, None, _)) => normalize(&default).into(),
        _ => "guarded".into(),
    })
}
pub fn switch_tx(
    db: &Transaction<'_>,
    task: &str,
    mode: &str,
    sid: &str,
    origin: &str,
) -> CoreResult<()> {
    if !MODES.contains(&mode)
        || !matches!(
            origin,
            "acp_config_option"
                | "acp_set_mode"
                | "acp_pre_task_selection"
                | "runtime_session_default"
                | "native_owner_settings"
        )
    {
        return Err(CoreError::Refused("invalid_access_mode"));
    }
    let previous = current(db, task)?;
    let revision: i64 = db.query_row(
        "SELECT count(*)+1 FROM runtime_events WHERE task_id=?
        AND type IN ('NATIVE_ACCESS_INITIALIZED','NATIVE_ACCESS_SWITCHED')",
        [task],
        |r| r.get(0),
    )?;
    lifecycle::event_tx(
        db,
        task,
        if origin == "runtime_session_default" {
            "NATIVE_ACCESS_INITIALIZED"
        } else {
            "NATIVE_ACCESS_SWITCHED"
        },
        json!({"mode":mode,"previous":previous,"revision":revision,"session_id":sid,"origin":origin}),
    )?;
    Ok(())
}
pub fn auto_choice(request: &Value) -> Option<String> {
    let options = request["options"].as_array()?;
    for kind in ["allow_once", "allow_always"] {
        if let Some(id) = options
            .iter()
            .find(|o| o["kind"] == kind && o["optionId"].as_str().is_some_and(|s| !s.is_empty()))
            .and_then(|o| o["optionId"].as_str())
        {
            return Some(id.into());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_access_matches_ordered_scan_and_seeks_both_types() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(crate::store::schema::SQL).unwrap();
        db.execute_batch(
            "PRAGMA foreign_keys=OFF;
             WITH RECURSIVE sample(n, seed) AS (
                VALUES (1, 33) UNION ALL
                SELECT n+1, (seed*48271)%2147483647 FROM sample WHERE n<5000
             )
             INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at)
             SELECT 't'||(seed%23),
                CASE n%31 WHEN 1 THEN 'NATIVE_ACCESS_INITIALIZED'
                          WHEN 2 THEN 'NATIVE_ACCESS_SWITCHED' ELSE 'NOISE' END,
                'runtime',json_object('mode',CASE seed%4 WHEN 0 THEN 'auto'
                     WHEN 1 THEN 'guarded' WHEN 2 THEN 'development' ELSE 'unknown' END),'t'
             FROM sample;
             ANALYZE;",
        ).unwrap();
        for n in 0..24 {
            let task = format!("t{n}");
            let old: Option<String> = db.query_row(
                "SELECT json_extract(payload_json,'$.mode') FROM runtime_events WHERE task_id=?
                 AND type IN ('NATIVE_ACCESS_INITIALIZED','NATIVE_ACCESS_SWITCHED') ORDER BY id DESC LIMIT 1",
                [&task], |row| row.get(0)).optional().unwrap();
            assert_eq!(current(&db, &task).unwrap(), normalize(old.as_deref().unwrap_or("")));
        }
        let mut plan = db.prepare(&format!("EXPLAIN QUERY PLAN {CURRENT_SQL}")).unwrap();
        let uses = plan.query_map(["t0"], |row| row.get::<_, String>(3)).unwrap()
            .map(Result::unwrap)
            .filter(|detail| detail.contains("runtime_events_type") && detail.contains("type=? AND task_id=?"))
            .count();
        assert_eq!(uses, 2);
    }
}
