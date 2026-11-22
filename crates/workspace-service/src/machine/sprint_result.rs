//! Published evidence of a member of the authenticated Workspace sprint.
use super::{redact_display_text, sprint_graph::Scope, task_facts_git};
use cortex_core::{store::documents::Publisher, CoreError, CoreResult, Store};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

/// A historical verification observation remains tied to the plan revision
/// on which it ran. Never export command output or turn a missing run into a pass.
fn outcome(db: &Connection, task: &str) -> CoreResult<Value> {
    let (state, revision, synthesis, blocker): (String, i64, Option<String>, Option<String>) = db
        .query_row(
        "SELECT state,revision,synthesis,blocker FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let last:Option<(String,String)>=db.query_row(
        "SELECT type,payload_json FROM runtime_events WHERE task_id=? AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED')
         AND json_extract(payload_json,'$.revision')=? ORDER BY id DESC LIMIT 1", (task,revision), |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let verification = match last {
        Some((kind, raw)) => {
            let payload: Value = serde_json::from_str(&raw)?;
            let checks:Vec<_>=payload["checks"].as_array().into_iter().flatten().map(|v| json!({
                "receipt_ref":v["receipt_ref"],"program":v["argv"].get(0),"complete":v["complete"],"exit_code":v["exit_code"]
            })).collect();
            json!({"last_observation":kind,"revision":revision,"revision_digest":payload["revision_digest"],"checks":checks})
        }
        None => json!({"last_observation":null,"reason":"no_verification_for_this_revision"}),
    };
    let constraints = json!({"write_denials":cortex_core::lifecycle::steering::denials(db,task)?,
        "access_mode":cortex_core::access::current(db,task)?});
    Ok(
        json!({"state":state,"revision":revision,"synthesis":synthesis,"blocker":blocker,"verification":verification,
        "constraints":constraints,"decisions":decisions(db,task,None,16)?}),
    )
}

/// Question IDs and answers are evidence from the predecessor, not authority
/// for a new task. The cursor is confined to its runtime task and record type.
fn decisions(db: &Connection, task: &str, cursor: Option<&str>, limit: usize) -> CoreResult<Value> {
    let prefix = format!("sd.{task}.");
    let before = match cursor {
        Some(cursor) => cursor
            .strip_prefix(&prefix)
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|v| *v > 0)
            .ok_or(CoreError::Refused("invalid_cursor"))?,
        None => i64::MAX,
    };
    let mut statement = db.prepare(
        "SELECT rowid,id,text,answer_text FROM pending_questions
        WHERE task_id=? AND rowid<? AND status='answered' AND answer_text IS NOT NULL
        AND dedup_key NOT GLOB 'native_permission:*' ORDER BY rowid DESC LIMIT ?",
    )?;
    let rows = statement
        .query_map((task, before, (limit + 1) as i64), |r| {
            let question: String = r.get(2)?;
            let answer: String = r.get(3)?;
            Ok((
                r.get::<_, i64>(0)?,
                json!({"question_id":r.get::<_,String>(1)?,
            "question":redact_display_text(&question),"answer":redact_display_text(&answer)}),
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let next = (rows.len() > limit).then(|| format!("{prefix}{}", rows[limit - 1].0));
    let items: Vec<_> = rows
        .into_iter()
        .take(limit)
        .map(|(_, question)| question)
        .collect();
    Ok(json!({"items":items,"next_cursor":next}))
}

pub(crate) fn read(
    store: &Store,
    scope: &Scope,
    args: &Value,
    recipient: &std::path::Path,
) -> CoreResult<Value> {
    let id = args["member_id"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_arguments"))?;
    let member = scope.member(id)?;
    let cursor = args["cursor"].as_str();
    let limit = args["limit"].as_u64();
    let publisher = Publisher::new(store);
    if let Some(report) = args["report_id"].as_str() {
        let task = member
            .runtime
            .as_deref()
            .ok_or(CoreError::Refused("report_not_available"))?;
        let page = publisher.immutable_report_page(
            task,
            report,
            cursor,
            limit.unwrap_or(4000) as usize,
        )?;
        return Ok(
            json!({"member_id":id,"report":{"report_id":report,"markdown":page.markdown,"sha256":page.sha256,
            "total_characters":page.total_characters,"next_cursor":page.next_cursor}}),
        );
    }
    let mut value =
        json!({"member_id":id,"task":member.record,"closure":member.record["closedAs"]});
    let Some(task) = member.runtime.as_deref() else {
        if cursor.is_some() {
            return Err(CoreError::Refused("invalid_cursor"));
        }
        value["outcome"] = json!({"status":"unavailable","reason":"task_has_no_runtime_history"});
        value["code"] = json!({"status":"unknown","scope":"recorded_paths"});
        value["reports"] = json!({"items":[],"next_cursor":null});
        return Ok(value);
    };
    if cursor.is_some_and(|v| v.starts_with("sd.")) {
        let page = store.read(|db| decisions(db, task, cursor, limit.unwrap_or(16) as usize))?;
        return Ok(json!({"member_id":id,"decisions":page}));
    }
    let catalogue = publisher.immutable_reports(task, cursor, limit.unwrap_or(25) as usize)?;
    let items:Vec<_>=catalogue.reports.iter().map(|r|json!({
        "report_id":r.report_id,"title":r.title,"summary":r.summary,"sha256":r.sha256,"size_bytes":r.size_bytes
    })).collect();
    value["reports"] = json!({"items":items,"next_cursor":catalogue.next_cursor});
    let (result, code) = store.read(|db| {
        Ok((
            outcome(db, task)?,
            task_facts_git::sprint_availability(db, id, recipient)?,
        ))
    })?;
    value["outcome"] = result;
    value["code"] = code;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::{lifecycle::TaskMachine, OpenMode};

    #[test]
    fn result_binds_checks_to_revision_and_pages_work_decisions_without_permission_details() {
        let root =
            std::env::temp_dir().join(format!("sprint-result-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(
            crate::native_runtime::key(&root).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        TaskMachine::new(&store)
            .create("a", "Investigate the API", "owner")
            .unwrap();
        store.write(|db|{
            db.execute("UPDATE runtime_tasks SET revision=2,synthesis='Recorded API decision' WHERE task_id='a'",[])?;
            cortex_core::lifecycle::event_tx(db,"a","VERIFICATION_PASSED",json!({"revision":1}))?;
            for i in 0..20 {
                let id=format!("q{i}");
                db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,status,answer_text,created_at) VALUES (?,'a',?,'API format?','answered','plain text','now')",(&id,&id))?;
            }
            db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,status,answer_text,created_at) VALUES ('secret','a','native_permission:fixture','private permission body','answered','Allow once','now')",[])?;
            Ok(())
        }).unwrap();
        let first = store.read(|db| outcome(db, "a")).unwrap();
        assert_eq!(
            first["verification"]["reason"],
            "no_verification_for_this_revision"
        );
        assert!(!first.to_string().contains("private permission body"));
        let cursor = first["decisions"]["next_cursor"].as_str().unwrap();
        let rest = store
            .read(|db| decisions(db, "a", Some(cursor), 16))
            .unwrap();
        assert_eq!(rest["items"].as_array().unwrap().len(), 4);
        assert!(rest["next_cursor"].is_null());
        assert!(store
            .read(|db| decisions(db, "another", Some(cursor), 16))
            .is_err());
        store.write(|db|{
            cortex_core::lifecycle::event_tx(db,"a","VERIFICATION_FAILED",json!({"revision":2,"checks":[{"argv":["test"],"exit_code":1,"complete":true,"stderr":"private diagnostic"}]}))?;
            Ok(())
        }).unwrap();
        let checked = store.read(|db| outcome(db, "a")).unwrap();
        assert_eq!(
            checked["verification"]["last_observation"],
            "VERIFICATION_FAILED"
        );
        assert_eq!(checked["verification"]["checks"][0]["exit_code"], 1);
        assert!(!checked.to_string().contains("private diagnostic"));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
